//! Execution machinery: regions that own per-run and per-call allocations, the `Scope` that
//! holds dispatch tables and resolves calls, and the structured `KError` that propagates
//! failures. `kfunction` lives here because scope holds functions and functions capture scope.

pub(crate) mod bindings;
pub(crate) mod kerror;
pub(crate) mod kfunction;
mod lexical_frame;
mod scope;
mod scope_id;
pub(crate) mod seals;
mod statement_id;

#[cfg(test)]
mod tests;

pub use bindings::{
    BindingIndex, Bindings, BindingsReferenceFamily, DeclarationSite, FunctionLookup, Installer,
    MemberResolution, NameLookup, WriteGate,
};
pub use kerror::{KError, KErrorKind, TraceFrame};
pub(crate) use kerror::{location_from_expr, resolve_location};
pub(crate) use kfunction::action::{
    Action, ActionKind, AwaitContinue, BlockEntry, BlockRequest, BodyCtx, BodyPlacement, BoundArgs,
    CatchFn, DepPlacement, DepRequest, DepTerminal, FinishCtx, FramePlacement, SubDispatch,
    TailContract, require_bare_type_name, require_identifier_name, require_kexpression,
    require_ktype,
};
pub(crate) use kfunction::block_tail::{
    BlockBody, BlockScope, NoSeed, block_tail, freeze_body, fresh_cart_tail, seed,
};
pub(crate) use kfunction::body::{LeadingStatements, ReturnContract, body_statement_refs};
pub(crate) use kfunction::exec::{ExecFrame, ExecOutcome, PerCallReturn, run_user_fn, solved_type};
pub(crate) use kfunction::{
    ActionFn, Body, DeliveredFunction, KFunction, OpenedFunction, SealedFunction, WrapIndices,
};
pub use lexical_frame::{LexicalFrame, assemble_body_chain};
pub(crate) use scope::AdoptSeam;
pub(crate) use scope::HitTier;
pub use scope::Scope;
pub(crate) use scope::ViewMembers;
pub(crate) use scope::consolidate_object;
pub use scope::{RegionScopeFamily, ScopeRefFamily};
pub use scope_id::ScopeId;
pub(crate) use seals::{GroupSeal, OverloadSeal};
pub use statement_id::StatementId;
