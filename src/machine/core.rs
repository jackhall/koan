//! Execution machinery: regions that own per-run and per-call allocations, the `Scope` that
//! holds dispatch tables and resolves calls, and the structured `KError` that propagates
//! failures. `kfunction` lives here because scope holds functions and functions capture scope.

mod arena;
mod bindings;
mod kerror;
pub(crate) mod kfunction;
mod lexical_frame;
mod pending;
mod scope;
mod scope_id;
mod scope_ptr;

#[cfg(test)]
mod tests;

pub(crate) use arena::KoanRegionExt;
#[cfg(test)]
pub(crate) use arena::KoanRegionTestExt;
pub use arena::{CallFrame, FrameSet, FrameStorage, KoanRegion, RegionBrand, RegionTypeFamily};
pub use bindings::{
    ApplyOutcome, BindKind, BindingIndex, Bindings, FunctionLookup, MemberResolution, NameLookup,
    PendingBinderGuard, PendingTypeEntry, TypeHit, ValueHit,
};
pub(crate) use kerror::kerror_ktype;
pub use kerror::{KError, KErrorKind, TraceFrame};
pub use lexical_frame::{assemble_body_chain, LexicalFrame};
pub use scope::{Scope, ScopeKind};
pub use scope_id::ScopeId;
pub use scope_ptr::ScopeRefFamily;
