//! Execution machinery: regions that own per-run and per-call allocations, the `Scope` that
//! holds dispatch tables and resolves calls, and the structured `KError` that propagates
//! failures. `kfunction` lives here because scope holds functions and functions capture scope.

mod arena;
mod bindings;
mod carrier_witness;
mod kerror;
pub(crate) mod kfunction;
mod lexical_frame;
mod pending;
mod run_id;
mod scope;
mod scope_id;
mod scope_ptr;

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) use arena::KoanRegionTestExt;
pub use arena::{
    run_root_storage, CallFrame, FoldingBrand, FrameSet, FrameStorage, KoanRegion, RegionBrand,
    RegionTypeFamily, StepAllocator,
};
pub(crate) use arena::{FrameStorageExt, KoanRegionExt, KoanStorageProfile, Residence};
pub use bindings::{
    BindingIndex, Bindings, DeclarationSite, FunctionLookup, MemberResolution, NameLookup,
    NodeHandle, PendingBinderGuard, StoredReach,
};
pub(crate) use carrier_witness::force_substrate_borrows_host;
pub use carrier_witness::{CarrierWitness, DeliveredCarried};
pub(crate) use kerror::kerror_ktype;
pub use kerror::{KError, KErrorKind, TraceFrame};
pub(crate) use kfunction::action::{
    arg_held, arg_object, arg_type, arg_unresolved_type, require_bare_type_name,
    require_identifier_name, require_kexpression, require_ktype, scope_frame, Action,
    AwaitContinue, BlockEntry, BodyCtx, BodyPlacement, CatchContinue, DepPlacement, DepRequest,
    DepTerminal, FinishCtx, FramePlacement, OwnedDispatch, TailContract,
};
pub(crate) use kfunction::body::{
    body_statement_refs, split_body_statements, ReturnContract, SealedContract,
};
pub(crate) use kfunction::exec::{run_user_fn, ExecFrame, ExecOutcome, PerCallReturn};
pub(crate) use kfunction::{ActionFn, Body, ClassifiedSlots, KFunction, NodeId};
pub use lexical_frame::{assemble_body_chain, LexicalFrame};
pub use run_id::RunId;
pub use scope::Scope;
pub use scope_id::ScopeId;
pub use scope_ptr::ScopeRefFamily;
