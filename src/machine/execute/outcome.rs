//! The unified scheduler-step currency.
//!
//! Every node step — a fresh dispatch decide, a finish, a builtin body, an invoke — decides
//! against a read-only [`SchedulerView`](super::dispatch::SchedulerView) and **returns** an
//! [`Outcome`]; [`Scheduler::apply_outcome`](super::scheduler::Scheduler) is the sole place that
//! turns an outcome into the scheduler-graph writes it implies and the terminal
//! [`NodeStep`](super::nodes::NodeStep). The scheduler never learns *what* a step ran (dispatch /
//! invoke / builtin) nor *whether* it ran before — only a read view in and an outcome out.
//!
//! The taxonomy is three-way:
//! - [`Outcome::Done`] — the node dies, producing a value to lift or an error.
//! - [`Outcome::Continue`] — the node lives; replace its work and run again immediately (no park).
//! - [`Outcome::ParkThenContinue`] — park on deps; on resolve run a [`Continuation`] that yields
//!   another outcome.
//!
//! [`Outcome::Invoke`] is the dispatch→execution trigger: a decide picks a function but can't
//! acquire the per-call frame (a write), so it names the call and the harness acquires the frame
//! before running the pure `invoke` decide. [`Outcome::Redispatch`] is the one remaining
//! **transitional** variant — an immediate dispatch-specific re-decide via
//! [`KeywordedState::finish`](super::dispatch::keyworded::KeywordedState), shed when the
//! eager-subs re-resolve folds in.

use std::rc::Rc;

use crate::machine::core::kfunction::action::{Dep, DepPlacement, FramePlacement};
use crate::machine::core::kfunction::body::ReturnContract;
use crate::machine::core::kfunction::KFunction;
use crate::machine::core::{CallArena, ScopeId};
use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::{NodeId, TraceFrame};

use super::dispatch::DispatchState;
use super::nodes::{DispatchCombineFinish, NodeOutput, NodeWork};
use super::{CatchFinish, CombineFinish};

/// What a node's step wants the harness to do — the single currency every producer and finish
/// returns. See the module docs for the taxonomy.
// `Continue` is intrinsically the large variant (it carries `NodeWork` plus the
// frame/contract/chain tail-call payload), mirroring `NodeStep::Replace`; boxing the hot
// continuation path to balance variants is the wrong trade.
#[allow(clippy::large_enum_variant)]
pub(in crate::machine::execute) enum Outcome<'run> {
    /// The node dies with a value (to lift out of the dying frame) or an error.
    Done(NodeOutput<'run>),
    /// The node lives: install `work` and run again immediately (no park). `frame` rotates the
    /// per-call cart (`Inherit` keeps it; `ReuseReserve`/`FreshChild` install a new one — the
    /// harness resolves the placement to a cart); `contract` / `block_entry` / `body_index` carry
    /// the tail-call chain payload, all keep-first. `leading` are the body's non-tail statements:
    /// the harness dispatches them as siblings against the resolved frame before the tail-replace
    /// (so a decide stays write-free). Non-empty `leading` requires a `frame` that resolves to a
    /// cart; `body_index` already accounts for their count.
    Continue {
        work: NodeWork<'run>,
        frame: FramePlacement<'run>,
        contract: Option<ReturnContract<'run>>,
        block_entry: Option<ScopeId>,
        leading: Vec<KExpression<'run>>,
        body_index: usize,
    },
    /// Park the slot on `deps` and run `cont` when they resolve. `deps` layout is
    /// `[park_producers..., owned_subs...]`; `park_count` is the park-producer prefix length
    /// (`Notify` edges, kept alive), the suffix installs as `Owned` (cascade-freed). For a
    /// [`Continuation::Replay`] / [`Continuation::Forward`] every dep parks (notify-only).
    /// `dep_error_frame` is attached to a dep-error short-circuit (Combine-style) before the
    /// finish runs; `free` reclaims producers the decide phase consumed inline.
    ParkThenContinue {
        deps: Vec<DispatchDep<'run>>,
        park_count: usize,
        cont: Continuation<'run>,
        dep_error_frame: Option<TraceFrame>,
        free: Vec<usize>,
    },
    /// Run a resolved call. The dispatch→execution trigger: a decide that picks a function can't
    /// acquire the per-call frame (a write), so it names the call here and the harness acquires the
    /// frame (for a user fn) before calling the pure `invoke` decide. `free` reclaims eager-subs
    /// `Reuse` producers consumed inline. Not transitional — frame acquisition is an irreducible
    /// harness write, and this is its trigger.
    Invoke {
        picked: &'run KFunction<'run>,
        working_expr: KExpression<'run>,
        free: Vec<usize>,
    },
    /// Transitional: re-resolve dispatch against a fully-spliced `working_expr` immediately
    /// (the post-eager-subs continuation with no speculatively pre-picked function). `free`
    /// reclaims `Reuse` producers consumed inline.
    Redispatch {
        working_expr: KExpression<'run>,
        free: Vec<usize>,
    },
}

/// What a [`Outcome::ParkThenContinue`] runs once its deps resolve. The shapes are the closed set
/// of "what happens on wake":
/// - `Finish` consumes the resolved dep values and returns another [`Outcome`] — a dispatch decide
///   re-park/splice (its finish may itself re-park, so it lands as `NodeWork::DispatchCombine`).
/// - `Combine` is the action-harness combine ([`run_action`](super::harness::run_action)'s
///   `Action::Combine`): the slot becomes a `NodeWork::Combine` and its finish runs against a
///   read-only [`SchedulerView`].
/// - `Catch` is the action-harness catch ([`run_action`](super::harness::run_action)'s
///   `Action::Catch`): the slot becomes a `NodeWork::Catch` watching the realized `watched` dep;
///   the harness owns that producer. `watched`'s placement is realized at apply time (an `InScope`
///   watched enters a fresh single-statement block, unlike a Combine body's fan-out).
/// - `Replay` re-runs the parked dispatch decide (the `ParkSelf` shape — its payload becomes a
///   resume closure once `DispatchState` dissolves).
/// - `Forward` makes the slot *be* a single producer's value (the bare-name `Lift` forward).
pub(in crate::machine::execute) enum Continuation<'run> {
    Finish(DispatchCombineFinish<'run>),
    Combine(CombineFinish<'run>),
    Catch {
        watched: Dep<'run>,
        finish: CatchFinish<'run>,
    },
    Replay(DispatchState<'run>),
    Forward(NodeId),
}

/// A dependency a [`Outcome::ParkThenContinue`] declares. `Dispatch`/`*Lit` are fresh sub-slots
/// the harness submits (and owns); `Existing` is a pre-existing producer the decide phase found
/// that the slot merely parks on. Deps resolve in declaration order, so a finish reads
/// `results[k]` for the k-th dep — except an `InScope`-placed `Dispatch`, whose multi-statement
/// body fans out to one resolved producer per statement (the harness `extend`s them in order).
pub(in crate::machine::execute) enum DispatchDep<'run> {
    Dispatch {
        expr: KExpression<'run>,
        placement: DepPlacement<'run>,
    },
    ListLit(Vec<ExpressionPart<'run>>),
    DictLit(Vec<(ExpressionPart<'run>, ExpressionPart<'run>)>),
    RecordLit(Vec<(String, ExpressionPart<'run>)>),
    /// A deferred-return FN's first-call body: dispatch `statements` (its non-tail body + the
    /// return-type expression, in that order) as body-chain siblings in the freshly acquired
    /// per-call `frame`, fanning out to one owned producer per statement. The combine reads the
    /// last (the resolved return type) to build the `PerCall` contract; the earlier statements'
    /// scope binds feed the tail body. The only dep that carries its own frame.
    BodyBlock {
        frame: Rc<CallArena>,
        statements: Vec<KExpression<'run>>,
    },
    Existing(NodeId),
}
