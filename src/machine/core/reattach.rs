//! [`pin_deref`] — re-borrow a raw `*const T` whose pointee a heap pin (a frame `Rc`, the owning
//! region) holds fixed in place. The self-referential region-pointer derefs (the per-call region and
//! its escape frame, the functor-result region pin) that the [`Erased`](crate::scheduler::Erased)
//! erase/reattach discipline can't express, because the pointer, not its lifetime, is what's being
//! recovered.
//!
//! The generic erase-to-`'static` / reattach-to-`'r` machinery every lifetime-free *value* carrier
//! shares lives in the scheduler ([`crate::scheduler`]) — moving a value between nodes is the
//! scheduler's job. [`ScopePtr`](super::scope_ptr::ScopePtr) is that discipline specialized to a
//! `Scope` pointer with an invariance brand.

/// Materialize a `&'x T` from a raw `*const T` whose pointee a heap pin keeps fixed in place for
/// `'x` — the audited home for the self-referential `Rc<CallFrame>` region-pointer derefs (the
/// per-call region and its escape frame) and the functor-result region pin. Distinct from the
/// `Reattachable` retypes in the scheduler: those move a *value* between lifetimes; this re-borrows
/// a pointer whose pointee an owning `Rc` (or the frame holding it) cannot relocate or drop while
/// borrowed.
///
/// # Safety
///
/// `ptr` must be non-null, aligned, and point at a live, initialized `T` for all of `'x`; the caller
/// holds the pin (the frame `Rc`, the owning region) across the borrow. `'x` is driven by the
/// return-type annotation, not a turbofish argument.
pub(crate) unsafe fn pin_deref<'x, T: ?Sized>(ptr: *const T) -> &'x T {
    // SAFETY: see the function contract — the caller's held pin keeps the pointee live for `'x`.
    unsafe { &*ptr }
}
