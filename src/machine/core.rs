//! Execution machinery: regions that own per-run and per-call allocations, the `Scope` that
//! holds dispatch tables and resolves calls, and the structured `KError` that propagates
//! failures. `kfunction` lives here because scope holds functions and functions capture scope.

mod arena;
pub(crate) mod bindings;
mod carrier_witness;
mod kerror;
pub(crate) mod kfunction;
mod lexical_frame;
/// The reach-tightness report — the over-pinning audit at the fold chokepoint, compiled only under
/// the `region-audit` gate. Its body lives in [`audit/`](../../audit/README.md), the home for
/// measurement code that no build ships; what stays in `src/` is this declaration and the hooks
/// inside [`StepAllocator::alloc_carried_with`](arena::StepAllocator), which are the fold moment
/// itself and so cannot move.
#[cfg(any(test, feature = "region-audit"))]
#[path = "../../audit/reach_audit.rs"]
pub mod reach_audit;
mod ref_carriers;
mod scope;
mod scope_id;
mod statement_id;

#[cfg(test)]
mod tests;

pub use arena::{
    CallFrame, FoldingBrand, FrameCoverage, FrameReach, FrameStorage, KoanRegion, ProgramBrand,
    ProgramStorage, RegionBrand, RegionTypeFamily, RunWriter, StepAllocator, SubstrateDoor,
    program_storage, run_root_storage,
};
pub(crate) use arena::{FrameStorageExt, KoanRegionExt, KoanStorageProfile};
pub use bindings::{
    BindingIndex, Bindings, DeclarationSite, FunctionLookup, Installer, MemberResolution,
    NameLookup, WriteGate,
};
pub use carrier_witness::{
    CarrierWitness, DeliveredCarried, DeliveredFunction, DeliveredOperatorGroup, OpenedFunction,
    SealedFunction, SealedOperatorGroup, SplicedCell,
};
pub(crate) use carrier_witness::{GroupSeal, OverloadSeal, product_reaches_region, read_resting};
pub use kerror::{KError, KErrorKind, TraceFrame};
pub(crate) use kerror::{kerror_ktype, resolve_location};
pub(crate) use kfunction::action::{
    Action, ActionKind, AwaitContinue, BlockEntry, BlockRequest, BodyCtx, BodyPlacement, BoundArgs,
    CatchFn, DepPlacement, DepRequest, DepTerminal, FinishCtx, FramePlacement, SubDispatch,
    TailContract, require_bare_type_name, require_identifier_name, require_kexpression,
    require_ktype, scope_frame,
};
pub(crate) use kfunction::block_tail::{
    BlockBody, BlockScope, NoSeed, block_tail, freeze_body, fresh_cart_tail, seed,
};
pub(crate) use kfunction::body::{LeadingStatements, ReturnContract, body_statement_refs};
pub(crate) use kfunction::exec::{ExecFrame, ExecOutcome, PerCallReturn, run_user_fn};
pub(crate) use kfunction::{ActionFn, Body, ClassifiedSlots, KFunction};
pub use lexical_frame::{LexicalFrame, assemble_body_chain};
pub use ref_carriers::{ModuleRefFamily, ScopeRefFamily};
pub(crate) use scope::AdoptSeam;
pub use scope::Scope;
pub use scope_id::ScopeId;
pub use statement_id::StatementId;
