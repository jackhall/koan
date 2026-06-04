//! `ScopePtr` — the single audited owner of the `Scope` lifetime-erasure that
//! arena-stored carriers rely on. `CallArena`, `Module`, `Signature`, and `KFunction`
//! each hold a captured/defining scope whose real lifetime the borrow checker can't
//! track across the arena's `'static` storage; they store a `ScopePtr` and re-attach a
//! lifetime on access through the one [`ScopePtr::reattach`] method, so the transmute
//! and the cast that erases it live in one place instead of at every carrier.
//!
//! See [memory-model.md § Arena lifetime erasure](../../../design/memory-model.md#arena-lifetime-erasure)
//! for the soundness argument the carriers' pinning supplies.

use std::ptr::NonNull;

use super::scope::Scope;

/// A `Scope` pointer with its lifetime erased to `'static` for storage in a lifetime-free
/// (`CallArena`) or self-referential (`Module` / `Signature` / `KFunction`) carrier.
///
/// `ScopePtr` is non-generic and proves *nothing* about the pointee's lifetime: the
/// carrying type's `Rc`/arena pinning supplies the real liveness guard, and each carrier
/// keeps its own `PhantomData` to pin its `'a` invariance (this newtype is covariant by
/// construction — never let it become a carrier's variance source).
#[derive(Clone, Copy)]
pub struct ScopePtr(NonNull<Scope<'static>>);

impl ScopePtr {
    /// Erase a live scope borrow to a storable `'static` pointer.
    pub fn erase(scope: &Scope<'_>) -> Self {
        // `Scope` is invariant in `'a`, so the through-`'static` cast is required.
        #[allow(clippy::unnecessary_cast)]
        let ptr = scope as *const Scope<'_> as *const Scope<'static>;
        // Non-null: `ptr` is derived from a reference.
        ScopePtr(unsafe { NonNull::new_unchecked(ptr as *mut Scope<'static>) })
    }

    /// Re-attach a caller-chosen `'a` to the stored scope. The carrier picks `'a` via its
    /// accessor's return type (its own lifetime parameter, or a receiver-bounded borrow).
    ///
    /// SAFETY: the pointee is arena-allocated; the caller's carrier holds the `Rc`/arena
    /// pinning that keeps that storage alive, so `'a` is sound as long as it does not
    /// outlive the carrier's liveness witness. This is the lifetime-fabrication the whole
    /// arena model is built on — the one transmute every scope re-attach routes through.
    pub unsafe fn reattach<'a>(&self) -> &'a Scope<'a> {
        std::mem::transmute::<&Scope<'static>, &'a Scope<'a>>(self.0.as_ref())
    }
}
