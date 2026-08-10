//! Machine — the runtime that maps a parsed `KExpression` to a value by selecting the
//! `KFunction` whose signature matches its parts and running its `Body`. Submodules:
//!
//! - `core` — `Scope`, `KoanRegion`, `KError`, scheduler glue, and the
//!   `kfunction` submodule (`KFunction`, `Body`).
//! - `model` — `KType`, `KObject`, `Module`, `SigSchema`, signature traits.
//! - `execute` — top-level interpret loop and scheduler driver.

pub(crate) mod core;
pub(crate) mod execute;
pub mod model;

pub(crate) use core::kfunction::Body;
pub use core::kfunction::{KFunction, NodeId};
pub(crate) use core::{
    Action, ActionFn, AwaitContinue, BlockBody, BlockEntry, BlockScope, BlockSeed, BodyCtx,
    CatchContinue, DepPlacement, DepRequest, DepTerminal, FinishCtx, FoldingBrand, FramePlacement,
    GroupSeal, OverloadSeal, OwnedDispatch, ReturnContract, StepAllocator, TailContract, arg_held,
    arg_object, arg_type, arg_unresolved_type, block_tail, body_statement_refs, kerror_ktype,
    require_bare_type_name, require_identifier_name, require_kexpression, require_ktype,
    split_body_statements,
};
pub(crate) use core::{
    AdoptSeam, BindingIndex, CallFrame, CarrierWitness, DeclarationSite, FrameCoverage, KoanRegion,
    LexicalFrame, MemberResolution, NameLookup, NodeHandle, RegionTypeFamily, RunId, RunWriter,
    TraceFrame,
};
pub use core::{
    Bindings, DeliveredCarried, DeliveredOperatorGroup, FrameStorage, KError, KErrorKind,
    OpenedFunction, ProgramBrand, ProgramStorage, Scope, ScopeId, SealedFunction,
    SealedOperatorGroup, SplicedCell, WriteGate, program_storage, run_root_storage,
};
pub(crate) use execute::seed_run_root;
pub(crate) use execute::{
    BrandCompose, DispatchOutcome, FieldListDeferral, NameOutcome, StepCarried, build_type_operand,
    seal_type_identity,
};
pub use execute::{KoanRuntime, interpret, interpret_with_writer, interpret_with_writer_path};
