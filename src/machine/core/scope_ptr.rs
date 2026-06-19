//! `ScopePtr` — the single audited owner of the `Scope` lifetime-erasure that
//! arena-stored carriers rely on. `CallFrame`, `Module`, `ModuleSignature`, and `KFunction`
//! each hold a captured/defining scope whose real lifetime the borrow checker can't
//! track across the arena's `'static` storage; they store a `ScopePtr<'a>` and re-attach
//! the scope on access, so the transmute and the cast that erases it live in one place
//! instead of at every carrier.
//!
//! `ScopePtr<'a>` is *branded* with the carrier's lifetime: [`ScopePtr::erase`] consumes a
//! real `&'a Scope<'a>` and records that `'a` in `_brand`, so [`ScopePtr::reattach`] re-hands
//! the same `'a` back as a **safe** method — the brand bounds every use of the pointer (and
//! of the carrier holding it) to a lifetime the original borrow already proved live. The
//! three carriers that own a real `'a` — `Module`, `ModuleSignature`, `KFunction` — re-attach
//! through `reattach` and carry no `unsafe`.
//!
//! Two lifetime-free carriers store a `ScopePtr<'static>` and must fabricate the content lifetime
//! back, because neither can brand it: `CallFrame` (non-generic — it backs `Rc<CallFrame>` and
//! carries no lifetime) shortens to an `&self`-bounded lifetime through the `unsafe`
//! [`ScopePtr::reattach_unbounded`]; a scheduler node's `NodeScope::YokedChild` — a cart-ancestor
//! block scope evicted off the lifetime-free node — re-attaches a free content lifetime behind
//! an `&self`-bounded borrow through the `unsafe` [`ScopePtr::reattach_bounded`]. Both reach the
//! `'static` store through [`ScopePtr::erase_static`], a brand-dropping constructor that is *safe*
//! to call (forgetting a lifetime cannot fabricate one); the fabrication hazard is deferred to the
//! `unsafe` re-attach, witnessed by the carrier's pinning — the frame `Rc` (which, for a
//! `YokedChild`, pins the ancestor arena via its `FrameStorage.outer` chain). The carriers that own a real `'a` — `Module`, `ModuleSignature`,
//! `KFunction` — route the safe [`ScopePtr::reattach`] and carry no `unsafe`.
//!
//! See [memory-model.md § Arena lifetime erasure](../../../design/memory-model.md#arena-lifetime-erasure)
//! for the soundness argument the carriers' pinning supplies.

use std::marker::PhantomData;
use std::ptr::NonNull;

use super::scope::Scope;
use crate::scheduler::{reattach_ref, Reattachable};

/// `Reattachable` family for [`Scope`] — the family every scope-pointer re-attach (and the arena's
/// scope-erasure storage) routes through the single audited lifetime-retype. Layout-invariant: a
/// `Scope<'r>` is generic only in `'r`.
pub struct ScopeFamily;

// SAFETY: `Scope<'r>` is one type generic only in `'r`; its representation does not depend on `'r`.
unsafe impl Reattachable for ScopeFamily {
    type At<'r> = Scope<'r>;
}

/// A branded `Scope` pointer: its lifetime is erased to `'static` for storage in a
/// lifetime-free (`CallFrame`) or self-referential (`Module` / `ModuleSignature` / `KFunction`)
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
        // Non-null by construction (from a reference); `cast` retags the pointee to `'static` for
        // storage (`Scope` is invariant, so the lifetime cannot coerce). No `unsafe`: the
        // fabrication hazard is deferred to the re-attach, not the store.
        ScopePtr {
            ptr: NonNull::from(scope).cast::<Scope<'static>>(),
            _brand: PhantomData,
        }
    }

    /// Erase a scope to a `'static`-branded pointer for storage in a lifetime-free carrier. Unlike
    /// [`Self::erase`], which records the borrow's `'a` for the safe [`Self::reattach`], this drops
    /// the brand to `'static`, so the caller commits to recovering the content lifetime through an
    /// `unsafe` re-attach ([`Self::reattach_unbounded`] / [`Self::reattach_bounded`]). Safe to
    /// *construct*: it only casts a live reference to a `'static` pointer (same cast as
    /// [`Self::erase`]); forgetting the lifetime cannot fabricate one. Used by `CallFrame` and by a
    /// scheduler node's `NodeScope::YokedChild`.
    pub fn erase_static(scope: &Scope<'_>) -> ScopePtr<'static> {
        // Non-null by construction; `cast` retags to `'static` (same store-side erasure as
        // [`Self::erase`]). Safe: forgetting the lifetime for storage cannot fabricate one.
        ScopePtr {
            ptr: NonNull::from(scope).cast::<Scope<'static>>(),
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
    /// SAFETY: the caller guarantees the pointee outlives `'b`. Used only by `CallFrame`,
    /// which stores a `ScopePtr<'static>` and must shorten it to an `&self`-bounded `'b` that
    /// the invariant brand cannot supply by safe coercion. The carriers that own a real `'a`
    /// route the safe [`Self::reattach`] instead.
    pub unsafe fn reattach_unbounded<'b>(&self) -> &'b Scope<'b> {
        reattach_ref::<ScopeFamily>(self.ptr.as_ref())
    }

    /// Re-attach with the borrow `'step` *bounded* by the `&'step self` receiver and the scope
    /// content `'b` left free (`'b: 'step`, implied by `&'step Scope<'b>` well-formedness). Unlike
    /// [`Self::reattach_unbounded`], which collapses borrow and content into one `'b`, this
    /// splits them — `'step` for the borrow, a free `'b` for the content — and hands back a
    /// reference that **cannot outlive the receiver borrow**: re-anchoring it longer than the
    /// pointer's witness is a compile error, not a fabrication. The free `'b` is the residual,
    /// frame-`Rc`-pinned content claim (the same erasure [`Self::reattach_unbounded`] already
    /// carries), reachable only behind the `'step` borrow.
    ///
    /// SAFETY: `self.ptr` points at a live `Scope` the caller's `Rc<CallFrame>` witness pins
    /// for all of `'step`; the returned borrow is bounded to `'step`, so it cannot escape that pin.
    /// `'step` is driven by the receiver, `'b` by the return-type annotation.
    pub unsafe fn reattach_bounded<'step, 'b: 'step>(&'step self) -> &'step Scope<'b> {
        reattach_ref::<ScopeFamily>(self.ptr.as_ref())
    }
}

/// A scope handle that can **only** be re-handed with a borrow bounded by the reader — never at
/// a free/unbounded lifetime. This is the [`Scope::outer`](super::scope::Scope) handle: a
/// frame-bounded child re-hands its (possibly frame-bounded) parent, content `'a`, borrow capped
/// at the reader (`get`). Distinct from [`ScopePtr`] precisely because it omits the unbounded
/// `reattach`: with no way to cash the free content `'a` except behind a reader-bounded borrow,
/// the constructor needs **no** borrow==content coupling. [`ScopePtr::erase`]'s coupling exists
/// only to keep its unbounded `reattach()` sound — which `CallFrame` still needs — so the two
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
    /// witness borrow `'b` may be shorter than the content `'a`, because [`Self::get`] only ever
    /// re-hands behind a reader-bounded borrow — the free `'a` is never cashed unbounded, so a
    /// shorter witness cannot fabricate a longer-lived reference. Safe by construction.
    pub fn erase<'b>(scope: &'b Scope<'a>) -> Self {
        BoundedScopePtr {
            // Non-null by construction; `cast` retags to `'static` for storage. Safe — the free
            // content `'a` is only ever cashed behind the reader-bounded [`Self::get`].
            ptr: NonNull::from(scope).cast::<Scope<'static>>(),
            _brand: PhantomData,
        }
    }

    /// Re-hand the scope with the borrow **bounded** to the `&'step self` receiver, content `'a`
    /// left free (`'a: 'step`). Re-anchoring longer than the receiver borrow is a compile error,
    /// not a fabrication.
    ///
    /// SAFETY: `self.ptr` points at a live `Scope` the owning scope chain's frame-`Rc` witness
    /// pins for all of `'step` (a parent outlives the frame-bounded child whose `outer` holds this);
    /// the returned borrow is capped at `'step`, so it cannot escape that pin. `'step` is driven by the
    /// receiver, `'a` by the return-type annotation.
    pub fn get<'step>(&'step self) -> &'step Scope<'a> {
        unsafe { reattach_ref::<ScopeFamily>(self.ptr.as_ref()) }
    }
}
