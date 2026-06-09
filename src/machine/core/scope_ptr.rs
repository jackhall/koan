//! `ScopePtr` — the single audited owner of the `Scope` lifetime-erasure that
//! arena-stored carriers rely on. `CallArena`, `Module`, `Signature`, and `KFunction`
//! each hold a captured/defining scope whose real lifetime the borrow checker can't
//! track across the arena's `'static` storage; they store a `ScopePtr<'a>` and re-attach
//! the scope on access, so the transmute and the cast that erases it live in one place
//! instead of at every carrier.
//!
//! `ScopePtr<'a>` is *branded* with the carrier's lifetime: [`ScopePtr::erase`] consumes a
//! real `&'a Scope<'a>` and records that `'a` in `_brand`, so [`ScopePtr::reattach`] re-hands
//! the same `'a` back as a **safe** method — the brand bounds every use of the pointer (and
//! of the carrier holding it) to a lifetime the original borrow already proved live. The
//! three carriers that own a real `'a` — `Module`, `Signature`, `KFunction` — re-attach
//! through `reattach` and carry no `unsafe`.
//!
//! The one irreducible `'static → 'a` fabrication lives at `CallArena`, which is non-generic
//! (it backs `Rc<CallArena>` and carries no lifetime), so it stores a `ScopePtr<'static>` and
//! must shorten that to an `&self`-bounded lifetime. That single boundary routes through the
//! `unsafe` [`ScopePtr::reattach_unbounded`], which fabricates a caller-chosen lifetime that
//! ignores the brand. It is the only `unsafe` re-attach in the model.
//!
//! See [memory-model.md § Arena lifetime erasure](../../../design/memory-model.md#arena-lifetime-erasure)
//! for the soundness argument the carriers' pinning supplies.

use std::marker::PhantomData;
use std::ptr::NonNull;

use super::scope::Scope;

/// A branded `Scope` pointer: its lifetime is erased to `'static` for storage in a
/// lifetime-free (`CallArena`) or self-referential (`Module` / `Signature` / `KFunction`)
/// carrier, while `_brand` records the `'a` the live borrow proved.
///
/// The `PhantomData<&'a Scope<'a>>` brand does two jobs. It bounds every use of the
/// `ScopePtr<'a>` — and of the carrier that holds it — to `'a`, making [`Self::reattach`] a
/// safe method. And because `Scope<'a>` is invariant in `'a`, the brand makes `ScopePtr<'a>`
/// (and each carrier) invariant in `'a`. Do **not** weaken `_brand` to a covariant
/// `PhantomData<&'a ()>`: a longer `'a` would then coerce in silently, and the next
/// `reattach` would hand out a reference outliving the pointee — a use-after-free.
#[derive(Clone, Copy)]
pub struct ScopePtr<'a> {
    ptr: NonNull<Scope<'static>>,
    _brand: PhantomData<&'a Scope<'a>>,
}

impl<'a> ScopePtr<'a> {
    /// Erase a live scope borrow to a storable `'static` pointer, recording the input's `'a`
    /// in the brand. Safe: it consumes a real `&'a Scope<'a>`, so it cannot fabricate a
    /// lifetime longer than the borrow already proved.
    pub fn erase(scope: &'a Scope<'a>) -> Self {
        // `Scope` is invariant in `'a`, so the through-`'static` cast is required.
        #[allow(clippy::unnecessary_cast)]
        let ptr = scope as *const Scope<'_> as *const Scope<'static>;
        // Non-null: `ptr` is derived from a reference.
        ScopePtr {
            ptr: unsafe { NonNull::new_unchecked(ptr as *mut Scope<'static>) },
            _brand: PhantomData,
        }
    }

    /// Re-attach the branded `'a` to the stored scope. Safe: [`Self::erase`] consumed a real
    /// `&'a Scope<'a>`, `_brand` bounds every use of this `ScopePtr<'a>` (and its carrier) to
    /// `'a`, and the arena keeps the pointee alive for all of `'a`, so handing back `'a` is
    /// exactly the lifetime the original borrow proved.
    pub fn reattach(&self) -> &'a Scope<'a> {
        // SAFETY: `'a` is the brand-recorded lifetime of a real `&'a Scope<'a>` the arena
        // keeps live for all of `'a`; re-attaching the same `'a` is sound. `'a` is driven by
        // the return-type annotation — `reattach_unbounded`'s lifetime is late-bound, so it
        // cannot be a turbofish argument.
        let reattached: &'a Scope<'a> = unsafe { self.reattach_unbounded() };
        reattached
    }

    /// Re-attach a caller-chosen `'b`, ignoring the brand. The single
    /// `transmute::<&Scope<'static>, &'b Scope<'b>>` in the model.
    ///
    /// SAFETY: the caller guarantees the pointee outlives `'b`. Used only by `CallArena`,
    /// which stores a `ScopePtr<'static>` and must shorten it to an `&self`-bounded `'b` that
    /// the invariant brand cannot supply by safe coercion. The carriers that own a real `'a`
    /// route the safe [`Self::reattach`] instead.
    pub unsafe fn reattach_unbounded<'b>(&self) -> &'b Scope<'b> {
        std::mem::transmute::<&Scope<'static>, &'b Scope<'b>>(self.ptr.as_ref())
    }

    /// Re-attach with the borrow `'p` *bounded* by the `&'p self` receiver and the scope
    /// content `'a` left free (`'a: 'p`, implied by `&'p Scope<'a>` well-formedness). Unlike
    /// [`Self::reattach_unbounded`], which collapses borrow and content into one `'b`, this
    /// hands back a reference that **cannot outlive the receiver borrow** — re-anchoring it
    /// longer than the pointer's witness is a compile error, not a fabrication. The free `'a`
    /// is the residual, frame-`Rc`-pinned content claim (the same erasure
    /// [`Self::reattach_unbounded`] already carries), reachable only behind the `'p` borrow.
    ///
    /// SAFETY: `self.ptr` points at a live `Scope` the caller's `Rc<CallArena>` witness pins
    /// for all of `'p`; the returned borrow is bounded to `'p`, so it cannot escape that pin.
    /// `'p` is driven by the receiver, `'a` by the return-type annotation.
    pub unsafe fn reattach_bounded<'p, 'c: 'p>(&'p self) -> &'p Scope<'c> {
        std::mem::transmute::<&'p Scope<'static>, &'p Scope<'c>>(self.ptr.as_ref())
    }
}

/// A scope handle that can **only** be re-handed with a borrow bounded by the reader — never at
/// a free/unbounded lifetime. This is the [`Scope::outer`](super::scope::Scope) handle: a
/// frame-bounded child re-hands its (possibly frame-bounded) parent, content `'a`, borrow capped
/// at the reader (`get`). Distinct from [`ScopePtr`] precisely because it omits the unbounded
/// `reattach`: with no way to cash the free content `'a` except behind a reader-bounded borrow,
/// the constructor needs **no** borrow==content coupling. [`ScopePtr::erase`]'s coupling exists
/// only to keep its unbounded `reattach()` sound — which `CallArena` still needs — so the two
/// handle types stay separate rather than relaxing one and reintroducing the fabrication hazard.
///
/// Invariant in `'a` for the same reason as [`ScopePtr`] (the `Scope<'a>` brand); do not weaken.
#[derive(Clone, Copy)]
pub struct BoundedScopePtr<'a> {
    ptr: NonNull<Scope<'static>>,
    _brand: PhantomData<&'a Scope<'a>>,
}

impl<'a> BoundedScopePtr<'a> {
    /// Erase a scope of content `'a` to a bounded handle. **No** borrow==content coupling: the
    /// witness borrow `'w` may be shorter than the content `'a`, because [`Self::get`] only ever
    /// re-hands behind a reader-bounded borrow — the free `'a` is never cashed unbounded, so a
    /// shorter witness cannot fabricate a longer-lived reference. Safe by construction.
    pub fn erase<'w>(scope: &'w Scope<'a>) -> Self {
        #[allow(clippy::unnecessary_cast)]
        let ptr = scope as *const Scope<'_> as *const Scope<'static>;
        BoundedScopePtr {
            // Non-null: derived from a reference.
            ptr: unsafe { NonNull::new_unchecked(ptr as *mut Scope<'static>) },
            _brand: PhantomData,
        }
    }

    /// Re-hand the scope with the borrow **bounded** to the `&'p self` receiver, content `'a`
    /// left free (`'a: 'p`). Re-anchoring longer than the receiver borrow is a compile error,
    /// not a fabrication.
    ///
    /// SAFETY: `self.ptr` points at a live `Scope` the owning scope chain's frame-`Rc` witness
    /// pins for all of `'p` (a parent outlives the frame-bounded child whose `outer` holds this);
    /// the returned borrow is capped at `'p`, so it cannot escape that pin. `'p` is driven by the
    /// receiver, `'a` by the return-type annotation.
    pub fn get<'p>(&'p self) -> &'p Scope<'a> {
        unsafe { std::mem::transmute::<&'p Scope<'static>, &'p Scope<'a>>(self.ptr.as_ref()) }
    }
}
