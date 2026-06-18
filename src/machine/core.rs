//! Execution machinery: arenas that own per-run and per-call allocations, the `Scope` that
//! holds dispatch tables and resolves calls, and the structured `KError` that propagates
//! failures. `kfunction` lives here because scope holds functions and functions capture scope.

mod arena;
mod bindings;
mod kerror;
pub(crate) mod kfunction;
mod lexical_frame;
mod pending;
mod reattach;
mod scope;
mod scope_id;
mod scope_ptr;
pub mod source;
mod storage_frame;

#[cfg(test)]
mod tests;

pub use arena::{CallArena, FrameStorage, RuntimeArena};
pub use bindings::{
    ApplyOutcome, BindingIndex, Bindings, FunctionLookup, PendingBinderGuard, PendingTypeEntry,
    Resolution,
};
pub(crate) use kerror::kerror_ktype;
pub use kerror::{KError, KErrorKind, TraceFrame};
pub use lexical_frame::{assemble_body_chain, LexicalFrame};
pub use scope::{KFuture, Scope, ScopeKind};
pub use scope_id::ScopeId;
pub use scope_ptr::ScopePtr;
