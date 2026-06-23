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
//! - [`ErasedScopePtr`] — the **one audited `unsafe`** the whole scope-erasure surface reduces to,
//!   for the two carriers that hold *no* lifetime and so cannot brand `'a`:
//!   [`CallFrame`](super::arena::CallFrame)'s per-call child scope (non-generic — it backs
//!   `Rc<CallFrame>`) and a scheduler node's `NodeScope::YokedChild` (a cart-ancestor block scope
//!   evicted off a lifetime-free node). Both fabricate the content lifetime back on read; the
//!   witness is **external** (the frame `Rc`, which for a `YokedChild` pins the ancestor region via
//!   its `FrameStorage.outer` chain) and not expressible in the type, so
//!   [`ErasedScopePtr::reattach`] is `unsafe`. Store-side ([`ErasedScopePtr::erase`]) stays safe —
//!   forgetting a lifetime for storage cannot fabricate one.
//!
//! See [memory-model.md § Arena lifetime erasure](../../../design/memory-model.md#region-lifetime-erasure)
//! for the soundness argument the carriers' pinning supplies.

use std::marker::PhantomData;
use std::ptr::NonNull;

use super::scope::Scope;
use crate::scheduler::reattach_ref;
use crate::witnessed::reattachable;

/// `Reattachable` family for [`Scope`] — the family every scope-pointer re-attach (and the region's
/// scope-erasure storage) routes through the single audited lifetime-retype. Layout-invariant: a
/// `Scope<'r>` is generic only in `'r`.
pub struct ScopeFamily;

reattachable!(ScopeFamily => Scope<'r>);

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
/// [`BoundedScopePtr`] / [`ScopePtr`], whose safe re-hand is witnessed by a real `'a` the carrier
/// structurally proves, this handle's witness is **external** (the frame `Rc`, which for a
/// `YokedChild` pins the ancestor region via its `FrameStorage.outer` chain) and not expressible in
/// the type — so the re-attach is the one consolidated `unsafe` the whole scope-erasure surface
/// reduces to.
///
/// Store-side ([`Self::erase`]) is safe — forgetting a lifetime for storage cannot fabricate one;
/// the fabrication hazard is concentrated entirely in the single [`Self::reattach`].
#[derive(Clone, Copy)]
pub struct ErasedScopePtr {
    ptr: NonNull<Scope<'static>>,
}

impl ErasedScopePtr {
    /// Erase a live scope borrow to a storable `'static`-typed pointer for a lifetime-free carrier.
    /// Safe to construct: it only casts a live reference to a `'static` pointer (`Scope` is
    /// invariant, so the lifetime cannot coerce); forgetting the lifetime for storage cannot
    /// fabricate one. The caller commits to recovering the content lifetime through the `unsafe`
    /// [`Self::reattach`], witnessed by the carrier's pinning.
    pub fn erase(scope: &Scope<'_>) -> Self {
        ErasedScopePtr {
            ptr: NonNull::from(scope).cast::<Scope<'static>>(),
        }
    }

    /// Re-attach with the borrow `'step` *bounded* by the `&'step self` receiver and the scope
    /// content `'b` left free (`'b: 'step`, implied by `&'step Scope<'b>` well-formedness). The
    /// returned reference **cannot outlive the receiver borrow**, so re-anchoring it past the
    /// pointer's witness is a compile error; the free `'b` is the residual content claim the
    /// external witness (the held frame `Rc`) pins. A caller wanting the collapsed
    /// borrow==content form (`&'a Scope<'a>`) instantiates `'b = 'step`.
    ///
    /// SAFETY: `self.ptr` points at a live `Scope` the caller's held witness — the frame `Rc`
    /// (which, for a `YokedChild`, pins the ancestor region through `FrameStorage.outer`) — keeps
    /// alive for all of `'step`; the returned borrow is bounded to `'step`, so it cannot escape that
    /// pin. `'step` is driven by the receiver, `'b` by the return-type annotation.
    pub unsafe fn reattach<'step, 'b: 'step>(&'step self) -> &'step Scope<'b> {
        reattach_ref::<ScopeFamily>(self.ptr.as_ref())
    }
}
