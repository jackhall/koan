//! Tests for the ascription builtins (`:|` opaque, `:!` transparent), split by surface:
//!
//! - [`ascription`] — primitive ascription behaviors: transparent passthrough,
//!   missing-member errors, opaque type-minting, and a roadmap-example walkthrough.
//! - [`functor`] — functor integration (module-system stage 2): module-typed
//!   parameters, signature-bound dispatch, generative application.

mod ascription;
mod functor;
