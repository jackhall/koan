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
mod lexical_frame;
mod pending;
mod resolve_dispatch;
mod resolve_type_expr;
mod scope;
mod scope_id;
pub mod source;

#[cfg(test)]
mod tests;

pub use arena::{CallArena, RuntimeArena};
pub use bindings::{
    ApplyOutcome, BindingIndex, Bindings, FunctionLookup, PendingBinderGuard, PendingTypeEntry,
    Resolution,
};
pub use kerror::{Frame, KError, KErrorKind};
pub use lexical_frame::{assemble_body_chain, LexicalFrame};
pub use resolve_dispatch::{ResolveOutcome, Resolved};
pub use resolve_type_expr::ResolveTypeExprOutcome;
pub use scope::{KFuture, Scope, ScopeKind};
pub use scope_id::ScopeId;
