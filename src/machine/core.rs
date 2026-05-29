//! Execution machinery: arenas that own per-run and per-call allocations, the `Scope` that
//! holds dispatch tables and resolves calls, and the structured `KError` that propagates
//! failures. `kfunction` lives here because scope holds functions and functions capture scope.

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
pub use resolve_dispatch::{NameOutcome, ResolveOutcome, Resolved};
#[cfg(test)]
pub use resolve_dispatch::{reset_resolve_dispatch_entry_count, resolve_dispatch_entry_count};
pub use resolve_type_expr::{coerce_type_token_value, ResolveTypeExprOutcome};
pub use scope::{KFuture, Scope, ScopeKind};
pub use scope_id::ScopeId;
