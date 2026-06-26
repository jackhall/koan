//! The two audited `Scope`-pointer handles every region-stored carrier shares — the single home
//! for the `Scope` lifetime-erasure the borrow checker can't track across the region's `'static`
//! storage. A carrier holds a captured/defining scope whose real lifetime is gone once it lands in
//! `'static` storage; it re-attaches on access through one of these handles, so the cast that
//! erases the lifetime and the re-attach that recovers it live in one place rather than at every
//! carrier.
//!
//! The handles split on **whether the carrier can brand the scope's `'a`**:
//!
//! - [`BoundedScopePtr<'a>`] — the **safe** handle for carriers that own a real `'a`:
//!   [`Scope::outer`](super::scope::Scope), [`KFunction`](super::kfunction::KFunction),
//!   [`Module`](crate::machine::model::values::Module) /
//!   [`ModuleSignature`](crate::machine::model::values::ModuleSignature). The `PhantomData<&'a
//!   Scope<'a>>` brand records the content `'a` the live borrow proved (and keeps the carrier
//!   invariant in `'a`); [`BoundedScopePtr::get`] re-hands behind a reader-bounded borrow, so the
//!   free content `'a` is never cashed unbounded and the re-hand carries **no `unsafe`**. Do **not**
//!   weaken the brand to a covariant `PhantomData<&'a ()>`: a longer `'a` would coerce in silently
//!   and the next `get` would outlive the pointee.
//!
//! - [`ErasedScopePtr`] — the lifetime-free handle for the two carriers that hold *no* lifetime and
//!   so cannot brand `'a`: [`CallFrame`](super::arena::CallFrame)'s per-call child scope
//!   (non-generic — it backs `Rc<CallFrame>`) and a scheduler node's `NodeScope::YokedChild` (a
//!   cart-ancestor block scope evicted off a lifetime-free node). Both fabricate the content lifetime
//!   back on read; the witness is **external** (the frame `Rc`, which for a `YokedChild` pins the
//!   ancestor region via its `FrameStorage.outer` chain) and not expressible in the carrier's type.
//!   So the re-hand takes that pin as an explicit [`Witness`] borrow
//!   ([`ErasedScopePtr::reattach_witnessed`], the scope analog of
//!   [`reattach_with`](crate::witnessed)): the lone `unsafe` is the `NonNull` deref inside that one
//!   safe-signature method, and call sites carry none. Store-side ([`ErasedScopePtr::erase`]) stays
//!   safe — forgetting a lifetime for storage cannot fabricate one.
//!
//! See [memory-model.md § Arena lifetime erasure](../../../design/memory-model.md#region-lifetime-erasure)
//! for the soundness argument the carriers' pinning supplies.

use std::marker::PhantomData;
use std::ptr::NonNull;

use super::scope::Scope;
use crate::scheduler::{reattach_ref, reattach_ref_with};
use crate::witnessed::{erase_to_static, reattachable, Witness};

/// `Reattachable` family for [`Scope`] — the family every scope-pointer re-attach (and the region's
/// scope-erasure storage) routes through the single audited lifetime-retype. Layout-invariant: a
/// `Scope<'r>` is generic only in `'r`.
pub struct ScopeFamily;

reattachable!(ScopeFamily => Scope<'r>);

/// `Reattachable` family for a **reference** to a [`Scope`] — `&'r Scope<'r>`. It lets a borrowed
/// scope erase to a `&'static Scope` through the safe [`erase_to_static`] (the store side of the
/// lifetime-free [`ErasedScopePtr`] and the frame's externally-witnessed scope carrier), so the
/// erasure carries no `unsafe` cast. Layout-invariant: `&'r Scope<'r>` is a thin pointer independent
/// of `'r`. Recovery routes [`ScopeFamily`] via [`reattach_ref_with`] (a `&'w Scope<'static>` →
/// `&'w Scope<'b>`), the two families sharing one `'static`-erased representation.
pub struct ScopeRefFamily;

reattachable!(ScopeRefFamily => &'r Scope<'r>);

/// A branded `Scope` pointer that can **only** be re-handed with a borrow bounded by the reader —
/// never at a free/unbounded lifetime. The carrier owns a real `'a` (e.g. [`KFunction<'a>`], a
/// frame-bounded child's [`Scope::outer`](super::scope::Scope)): the `_brand` records that `'a` and
/// keeps the carrier invariant in `'a`, while [`Self::get`] re-hands the content `'a` only behind a
/// reader-bounded borrow. Because the free content `'a` is never cashed unbounded, a shorter
/// witness borrow cannot fabricate a longer-lived reference, so the constructor needs **no**
/// borrow==content coupling and `get` is safe. Contrast [`ErasedScopePtr`], whose lifetime-free
/// carrier cannot brand `'a` and so falls to an `unsafe` re-attach.
///
/// Invariant in `'a` via the `Scope<'a>` brand; do not weaken to a covariant marker.
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

    /// Erase a scope of content `'long` to a handle branded at a **shorter** `'a` (`'long: 'a`).
    /// The brand under-claims the real content lifetime: [`Self::get`] only ever re-hands at the
    /// branded `'a`, ≤ the scope's real life, so a reader can never out-claim the pointee. Used by
    /// the per-call frame builder ([`Scope::child_for_frame`](super::scope::Scope::child_for_frame))
    /// to brand a longer-lived lexical parent at the fresh per-call region's lifetime, so the child
    /// needs no common lifetime with its parent. Safe by construction (pointer cast + phantom).
    pub fn erase_shortened<'b, 'long: 'a>(scope: &'b Scope<'long>) -> Self {
        BoundedScopePtr {
            ptr: NonNull::from(scope).cast::<Scope<'static>>(),
            _brand: PhantomData,
        }
    }

    /// Shorten an existing handle's brand from `'a` to `'short` (`'a: 'short`). The handle's stored
    /// pointer is unchanged; only the phantom brand narrows. Sound for the same reason as
    /// [`Self::erase_shortened`] — narrowing the brand under-claims the pointee's real life. Copies a
    /// parent's `root` handle into a shorter-lived per-call child.
    pub fn shortened<'short>(self) -> BoundedScopePtr<'short>
    where
        'a: 'short,
    {
        BoundedScopePtr {
            ptr: self.ptr,
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

/// The single audited home for the lifetime-free scope erasure the non-generic carriers rely on:
/// [`CallFrame`](super::arena::CallFrame)'s per-call child scope and a scheduler node's
/// `NodeScope::YokedChild` (a cart-ancestor block scope evicted off the lifetime-free node). Both
/// carriers hold no lifetime — `CallFrame` backs `Rc<CallFrame>`, a node is stored erased — so
/// neither can brand the scope's `'a`, and both fabricate the content lifetime back on read. Unlike
/// [`BoundedScopePtr`], whose safe re-hand is witnessed by a real `'a` the carrier
/// structurally proves, this handle's witness is **external** (the frame `Rc`, which for a
/// `YokedChild` pins the ancestor region via its `FrameStorage.outer` chain) and not expressible in
/// the carrier's type. Rather than fabricate the lifetime in-situ, the re-hand takes that pin as an
/// explicit [`Witness`] borrow ([`Self::reattach_witnessed`], the scope-pointer analog of
/// [`reattach_with`](crate::witnessed)), so the only `unsafe` is the `NonNull` deref inside that one
/// safe-signature method and call sites carry none.
///
/// Store-side ([`Self::erase`]) is safe — forgetting a lifetime for storage cannot fabricate one;
/// the fabrication hazard is concentrated entirely in the single [`Self::reattach_witnessed`].
#[derive(Clone, Copy)]
pub struct ErasedScopePtr {
    /// A `&'static Scope` into the arena, erased once on the store side through the safe
    /// [`erase_to_static`] and recovered through the witness-bounded [`reattach_ref_with`]. No
    /// `&mut Scope` exists in the crate (mutation is interior `RefCell`) and a stored reference
    /// survives `typed_arena` growth under tree borrows, so the carrier holds the reference outright
    /// — the re-hand needs no `as_ref`, and the handle carries **no `unsafe`**.
    stored: &'static Scope<'static>,
}

impl ErasedScopePtr {
    /// Erase a live scope borrow to a storable `&'static Scope` for a lifetime-free carrier. Safe to
    /// construct: [`erase_to_static`] only forgets the reference's lifetime for storage (`Scope` is
    /// invariant, so the lifetime cannot coerce), and forgetting a lifetime for storage cannot
    /// fabricate one. The caller commits to recovering the content lifetime through
    /// [`Self::reattach_witnessed`], passing the carrier's pin as an explicit witness.
    pub fn erase<'a>(scope: &'a Scope<'a>) -> Self {
        ErasedScopePtr {
            stored: erase_to_static::<ScopeRefFamily>(scope),
        }
    }

    /// Re-attach bounded by a held [`Witness`] borrow — the scope-pointer analog of
    /// [`reattach_with`](crate::witnessed). The borrow `'w` is bounded by the witness `&'w W` and the
    /// scope content `'b` is left free (`'b: 'w`); the returned reference **cannot outlive `'w`**, so
    /// it cannot escape the pin the witness holds. The stored `&'static Scope` is re-anchored through
    /// the witness-bounded [`reattach_ref_with`], so call sites that hold the pinning witness (a
    /// frame `Rc`, the owning region) — and this method itself — carry **no `unsafe`**: the Witnessed
    /// discipline applied to a scope pointer.
    ///
    /// The witness keeps the pointee's region live — for a `YokedChild` the cart's
    /// `FrameStorage.outer` chain, for a `CallFrame` its own storage `Rc` — and `'w` is bounded by
    /// the witness borrow, so the re-anchored view cannot outrun the pin.
    pub fn reattach_witnessed<'w, 'b: 'w, W: Witness>(&'w self, witness: &'w W) -> &'w Scope<'b> {
        reattach_ref_with::<ScopeFamily, W>(self.stored, witness)
    }
}
