//! Machine — the runtime that maps a parsed `KExpression` to a value by selecting the
//! `KFunction` whose signature matches its parts and running its `Body`. Submodules:
//!
//! - `core` — `Scope`, `RuntimeArena`, `KError`, `KFuture`, scheduler glue.
//! - `kfunction` — `KFunction`, `Body`, `BodyResult`, `ArgumentBundle`, scheduler handle.
//! - `execute` — top-level interpret loop and scheduler driver.
//!
//! The `pub use` block below is the curated public surface.

pub(crate) mod kfunction;
pub(crate) mod core;
pub(crate) mod execute;

pub use kfunction::{
    ArgumentBundle, Body, BodyResult, CombineFinish, KFunction, NodeId, SchedulerHandle,
};
pub(crate) use kfunction::substitute_params;
pub use core::{
    CallArena, Frame, KError, KErrorKind, KFuture, ResolveOutcome, Resolution, Resolved,
    RuntimeArena, Scope,
};
pub use execute::{Scheduler, interpret, interpret_with_writer};
