//! FN's functor surface: module-typed parameters, sharing constraints, and
//! dependent / deferred return types.
//!
//! - [`elaboration`] — scope-aware type elaboration of FN signatures.
//! - [`sharing`] — `WITH` sharing constraints on functor parameters and
//!   return types.
//! - [`per_call_param_bind`] — per-call parameter bind for module-typed
//!   parameters at dispatch time.
//! - [`deferred_return`] — return-type expressions that reference earlier
//!   parameters, resolved per-call.
//! - [`bare_type_token`] — bare builtin type tokens (`Number`, `Str`, `Bool`,
//!   `Null`) as `:Type`-typed arguments.
//! - [`module_head_in_type_position`] — a bare module name in type position lowers
//!   to the module's principal signature.

mod bare_type_token;
mod deferred_return;
mod elaboration;
mod module_head_in_type_position;
mod per_call_param_bind;
mod sharing;
