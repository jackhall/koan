//! Machine — the runtime that maps a parsed `KExpression` to a value by selecting the
//! `KFunction` whose signature matches its parts and running its `Body`. Submodules:
//!
//! - `core` — `Scope`, `KoanRegion`, `KError`, scheduler glue, and the
//!   `kfunction` submodule (`KFunction`, `Body`).
//! - `model` — `KType`, `KObject`, `Module`, `SigContent`, signature traits.
//! - `execute` — top-level interpret loop and scheduler driver.

pub(crate) mod core;
pub(crate) mod execute;
pub mod model;

pub(crate) use core::kfunction::Body;
pub use core::kfunction::{KFunction, NodeId};
#[cfg(test)]
pub(crate) use core::KoanRegionTestExt;
pub(crate) use core::{
    arg_held, arg_object, arg_type, arg_unresolved_type, body_statement_refs, kerror_ktype,
    require_bare_type_name, require_identifier_name, require_kexpression, require_ktype,
    split_body_statements, Action, ActionFn, AwaitContinue, BinderBucketFn, BinderNameFn,
    BlockEntry, BodyCtx, CatchContinue, DepPlacement, DepRequest, DepTerminal, FinishCtx,
    FoldingBrand, FramePlacement, FrameStorageExt, KoanRegionExt, KoanStorageProfile,
    ReturnContract, StepAllocator, StoredReach, TailContract,
};
pub use core::{
    run_root_storage, Bindings, DeliveredCarried, FrameStorage, KError, KErrorKind, Scope, ScopeId,
};
pub(crate) use core::{
    BindKind, BindingIndex, CallFrame, CarrierWitness, FrameSet, FunctionLookup, KoanRegion,
    LexicalFrame, MemberResolution, NameLookup, RegionBrand, RegionTypeFamily, TraceFrame,
};
pub(crate) use execute::{
    build_type_operand, defer_field_list_action, defer_field_list_action_composed,
    seal_type_identity, BrandCompose, DispatchOutcome, NameOutcome, StepCarried,
};
pub use execute::{interpret, interpret_with_writer, interpret_with_writer_path, KoanRuntime};
