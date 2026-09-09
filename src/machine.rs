//! Machine — the runtime that maps a parsed `KExpression` to a value by selecting the
//! `KFunction` whose signature matches its parts and running its `Body`.

pub(crate) mod core;
pub(crate) mod execute;
pub mod model;

pub use crate::scheduler::NodeId;
pub(crate) use core::kfunction::Body;
pub use core::kfunction::KFunction;
pub(crate) use core::{
    Action, ActionFn, AwaitContinue, BlockBody, BlockEntry, BlockRequest, BlockScope, BodyCtx,
    BoundArgs, DepPlacement, DepRequest, DepTerminal, FinishCtx, FramePlacement, GroupSeal, NoSeed,
    OverloadSeal, ReturnContract, SubDispatch, TailContract, block_tail, body_statement_refs,
    fresh_cart_tail, require_bare_type_name, require_identifier_name, require_kexpression,
    require_ktype, seed,
};
pub(crate) use core::{
    AdoptSeam, BindingIndex, DeclarationSite, HitTier, Installer, LexicalFrame, MemberResolution,
    NameLookup, TraceFrame,
};
pub use core::{Bindings, KError, KErrorKind, Scope, ScopeId, WriteGate};
pub use execute::ProducerId;
/// The reach-tightness report's reader surface — present only under the `region-audit` gate, which
/// is also what compiles the audit itself in.
#[cfg(any(test, feature = "region-audit"))]
pub use execute::reach_audit;
pub(crate) use execute::seed_run_root;
pub(crate) use execute::{
    DispatchOutcome, FieldListDeferral, StepAllocator, StepCarried, build_type_operand,
    seal_type_identity,
};
pub use execute::{KoanRuntime, interpret, interpret_with_writer, interpret_with_writer_path};
