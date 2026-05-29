//! Tests for the ascription builtins (`:|` opaque, `:!` transparent).
//!
//! - [`ascription`] — primitive behaviors: transparent passthrough, missing-member
//!   errors, opaque type-minting.
//! - [`functor`] — module-typed parameters, signature-bound dispatch, generative
//!   application.

mod ascription;
mod functor;
