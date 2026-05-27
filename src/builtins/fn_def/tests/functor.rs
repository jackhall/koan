//! Tests for FN's functor surface — FN with module-typed parameters,
//! sharing constraints, and dependent / deferred return types.
//!
//! - [`elaboration`] — scope-aware type elaboration of FN signatures
//!   (signature-bound params, LET→FN ordering, type-value bindings).
//! - [`sharing`] — `SIG_WITH` sharing constraints on functor parameters
//!   and return types (mismatch rejection, multi-slot pinning).
//! - [`per_call_type_side_bind`] — per-call type-side bind so functor bodies
//!   see the right `KType` for module-typed parameters at dispatch time.
//! - [`deferred_return`] — return-type expressions that reference earlier
//!   parameters (`MODULE_TYPE_OF p`, bare param name, `SIG_WITH p.T`),
//!   resolved per-call.
//! - [`bare_type_token`] — bare builtin type tokens (`Number`, `Str`,
//!   `Bool`, `Null`) as `:Type`-typed arguments (cut (a) of
//!   `roadmap/type_language/bare-type-token-functor-arg.md`).

mod bare_type_token;
mod deferred_return;
mod per_call_type_side_bind;
mod elaboration;
mod sharing;
