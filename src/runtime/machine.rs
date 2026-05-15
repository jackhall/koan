//! Machine — the runtime that maps a parsed `KExpression` to a value by selecting the
//! `KFunction` whose signature matches its parts and running its `Body`. Submodules:
//!
//! - `core` — `Scope`, `RuntimeArena`, `KError`, `KFuture`, scheduler glue, plus
//!   the `kfunction` submodule (`KFunction`, `Body`, `BodyResult`, `ArgumentBundle`,
//!   scheduler handle) — kept together because the two have bidirectional traffic
//!   that any top-level split spuriously fans out.
//! - `model` — `KType`, `KObject`, `Module`, `Signature`, signature traits.
//! - `execute` — top-level interpret loop and scheduler driver.
//!
//! The `pub use` block below is the curated public surface.

pub(crate) mod core;
pub(crate) mod execute;
pub mod model;

pub use core::kfunction::{
    ArgumentBundle, Body, BodyResult, CombineFinish, KFunction, NodeId, SchedulerHandle,
};
pub(crate) use core::kfunction::substitute_params;
pub use core::{
    Bindings, CallArena, Frame, KError, KErrorKind, KFuture, ResolveOutcome, Resolution,
    Resolved, RuntimeArena, Scope, ScopeKind,
};
pub use execute::{Scheduler, interpret, interpret_with_writer};
