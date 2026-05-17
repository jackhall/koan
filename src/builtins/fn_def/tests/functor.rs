//! Tests for FN's functor surface — FN with module-typed parameters,
//! sharing constraints, and dependent / deferred return types.
//!
//! - [`elaboration`] — scope-aware type elaboration of FN signatures
//!   (signature-bound params, LET→FN ordering, type-value bindings).
//! - [`sharing`] — `SIG_WITH` sharing constraints on functor parameters
//!   and return types (mismatch rejection, multi-slot pinning).
//! - [`dual_write`] — per-call type-side dual-write so functor bodies see
//!   the right `KType` for module-typed parameters at dispatch time.
//! - [`deferred_return`] — return-type expressions that reference earlier
//!   parameters (`MODULE_TYPE_OF p`, bare param name, `SIG_WITH p.T`),
//!   resolved per-call.

mod deferred_return;
mod dual_write;
mod elaboration;
mod sharing;
