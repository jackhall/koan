//! Execution machinery: arenas that own per-run and per-call allocations, the `Scope` that
//! holds dispatch tables and resolves calls, and the structured `KError` that propagates
//! failures. `Bindings` (the lexical binding façade) and `PendingQueue` (the deferred-write
//! queue) live in their own submodules to keep `scope.rs` focused on dispatch. The
//! `kfunction` submodule (`KFunction`, `Body`, `BodyResult`, `ArgumentBundle`, scheduler
//! handle) lives here because its types and the arena/scope types are bidirectionally
//! linked — scope holds functions, functions capture scope.

mod arena;
mod bindings;
mod kerror;
pub(crate) mod kfunction;
mod pending;
mod scope;

#[cfg(test)]
mod tests;

pub use arena::{CallArena, RuntimeArena};
pub use bindings::{ApplyOutcome, Bindings, PendingTypeEntry};
pub use kerror::{Frame, KError, KErrorKind};
pub use scope::{KFuture, ResolveOutcome, Resolution, Resolved, Scope, ScopeKind};
