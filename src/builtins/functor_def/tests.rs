//! Tests for the `FUNCTOR` builtin, split by theme:
//!
//! - [`binder`] — happy-path registration and the `is_functor` flag flips on
//!   the constructed `KFunction`, with `ktype()` projecting `KFunctor`.
//! - [`return_validation`] — definition-time validation of the FUNCTOR
//!   return-type slot, both Resolved and Deferred arms.
//! - [`recursive_carrier`] — curried-functor return slot (`-> :(Functor (...)
//!   -> SetSig)`) admits via the recursive `KFunctor` arm.

mod binder;
mod recursive_carrier;
mod return_validation;
