//! Execution machinery: regions that own per-run and per-call allocations, the `Scope` that
//! holds dispatch tables and resolves calls, and the structured `KError` that propagates
//! failures. `kfunction` lives here because scope holds functions and functions capture scope.

mod arena;
pub(crate) mod bindings;
mod carrier_witness;
mod kerror;
pub(crate) mod kfunction;
mod lexical_frame;
mod ref_carriers;
mod run_id;
mod scope;
mod scope_id;

#[cfg(test)]
mod tests;

pub use arena::{
    program_storage, run_root_storage, CallFrame, FoldingBrand, FrameCoverage, FrameReach,
    FrameStorage, KoanRegion, ProgramBrand, ProgramStorage, RegionBrand, RegionTypeFamily,
    StepAllocator, SubstrateDoor,
};
pub(crate) use arena::{FrameStorageExt, KoanRegionExt, KoanStorageProfile};
pub use bindings::{
    BindingIndex, Bindings, DeclarationSite, FunctionLookup, MemberResolution, NameLookup,
    NodeHandle, WriteGate,
};
pub(crate) use carrier_witness::{product_reaches_region, read_resting, OverloadSeal};
pub use carrier_witness::{
    CarrierWitness, DeliveredCarried, DeliveredOperatorGroup, OpenedFunction, SealedFunction,
    SealedOperatorGroup, SplicedCell,
};
pub(crate) use kerror::kerror_ktype;
pub use kerror::{KError, KErrorKind, TraceFrame};
pub(crate) use kfunction::action::{
    arg_held, arg_object, arg_type, arg_unresolved_type, require_bare_type_name,
    require_identifier_name, require_kexpression, require_ktype, scope_frame, Action, ActionKind,
    AwaitContinue, BlockEntry, BodyCtx, BodyPlacement, CatchContinue, DepPlacement, DepRequest,
    DepTerminal, FinishCtx, FramePlacement, OwnedDispatch, TailContract,
};
pub(crate) use kfunction::block_tail::{block_tail, BlockBody, BlockScope, BlockSeed};
pub(crate) use kfunction::body::{body_statement_refs, split_body_statements, ReturnContract};
pub(crate) use kfunction::exec::{run_user_fn, ExecFrame, ExecOutcome, PerCallReturn};
pub(crate) use kfunction::{ActionFn, Body, ClassifiedSlots, KFunction, NodeId};
pub use lexical_frame::{assemble_body_chain, LexicalFrame};
pub use ref_carriers::{ModuleRefFamily, ScopeRefFamily};
pub use run_id::RunId;
pub(crate) use scope::AdoptSeam;
pub use scope::Scope;
pub use scope_id::ScopeId;
