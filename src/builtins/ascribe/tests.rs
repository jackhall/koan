//! Tests for the ascription builtins (`:|` opaque, `:!` transparent).
//!
//! - [`ascription`] — primitive behaviors: transparent passthrough, missing-member
//!   errors, opaque type-minting.
//! - [`functor`] — module-typed parameters, signature-bound dispatch, generative
//!   application.
//! - [`self_sig`] — the self-sig a module / view carries, and satisfaction through the
//!   signature-subtyping relation.

mod ascription;
mod functor;
mod self_sig;
