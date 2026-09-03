//! Tests for the ascription builtins (`:|` opaque, `:!` transparent).
//!
//! - [`ascription`] — primitive behaviors: transparent passthrough, missing-member
//!   errors, opaque type-minting.
//! - [`cross_sig`] — dispatch specificity between two distinct `SIG`-declared signature
//!   types, ordered by structural `sig_subtype`.
//! - [`functor`] — module-typed parameters, signature-bound dispatch, generative
//!   application.
//! - [`self_sig`] — the self-sig a module / view carries, and satisfaction through the
//!   signature-subtyping relation.
//! - [`views`] — reads through an opaque view: every VAL member at the view's own per-call types,
//!   on every slot shape, with the barrier holding in both directions.
//! - [`nested`] — a signature nested inside a slot type: substitution, satisfaction and
//!   canonicalization through it, and the nested module born as a coerced view of itself.

mod ascription;
mod cross_sig;
mod functor;
mod nested;
mod self_sig;
mod views;
