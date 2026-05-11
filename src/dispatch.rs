//! Dispatch — the runtime that maps a parsed `KExpression` to a value by selecting the
//! `KFunction` whose signature matches its parts and running its `Body`. Submodules:
//!
//! - [`runtime`] — `Scope`, `RuntimeArena`, `KError`, `KFuture`, scheduler glue.
//! - [`types`] — `KType` and signature shapes (`Argument`, `SignatureElement`, ...).
//! - [`values`] — `KObject` and the heap-side value representation.
//! - [`kfunction`] — `KFunction`, `Body`, `BodyResult`, `ArgumentBundle`, scheduler handle.
//! - [`builtins`] — the registered builtin set and per-builtin test support.
//!
//! The `pub use` block below is the curated public surface — the ~18 names that most
//! callers need. Reach into a submodule directly only for symbols not re-exported here.

pub mod builtins;
pub mod kfunction;
pub mod runtime;
pub mod types;
pub mod values;

pub use kfunction::{
    ArgumentBundle, Body, BodyResult, CombineFinish, KFunction, NodeId, SchedulerHandle,
};
pub use runtime::{CallArena, Frame, KError, KErrorKind, KFuture, RuntimeArena, Scope};
pub use types::{Argument, ExpressionSignature, KType, Parseable, SignatureElement};
pub use values::KObject;
