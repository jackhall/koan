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
//! Three variants are **transitional** and shed by later phases of the scheduler-unification arc:
//! [`Outcome::Invoke`] / [`Outcome::Elaborate`] run a body holding `&mut Scheduler` (retired when
//! invoke/elaborate become `read-view → Outcome` producers), and [`Outcome::Redispatch`] is an
//! immediate dispatch-specific re-decide via
//! [`KeywordedState::finish`](super::dispatch::keyworded::KeywordedState).

use std::rc::Rc;

use crate::machine::core::kfunction::action::FramePlacement;
use crate::machine::core::kfunction::body::ReturnContract;
use crate::machine::core::kfunction::KFunction;
use crate::machine::core::ScopeId;
use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::{LexicalFrame, NodeId, TraceFrame};

use super::dispatch::DispatchState;
use super::nodes::{DispatchCombineFinish, NodeOutput, NodeWork};

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
    /// per-call cart (`Inherit` keeps it; `ReuseReserve`/`FreshChild` install a new one);
    /// `contract` / `block_entry` / `body_index` carry the tail-call chain payload, all keep-first.
    Continue {
        work: NodeWork<'run>,
        frame: FramePlacement<'run>,
        contract: Option<ReturnContract<'run>>,
        block_entry: Option<ScopeId>,
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
    /// Transitional (retired in P4): run the resolved call against `&mut Scheduler` and lower its
    /// body onto the slot. `free` reclaims eager-subs `Reuse` producers consumed inline.
    Invoke {
        picked: &'run KFunction<'run>,
        working_expr: KExpression<'run>,
        free: Vec<usize>,
    },
    /// Transitional (retired in P4): elaborate a structural record-type field list against
    /// `&mut Scheduler` and lower the resulting body onto the slot.
    Elaborate {
        fields: KExpression<'run>,
        chain: Option<Rc<LexicalFrame>>,
    },
    /// Transitional: re-resolve dispatch against a fully-spliced `working_expr` immediately
    /// (the post-eager-subs continuation with no speculatively pre-picked function). `free`
    /// reclaims `Reuse` producers consumed inline.
    Redispatch {
        working_expr: KExpression<'run>,
        free: Vec<usize>,
    },
}

/// What a [`Outcome::ParkThenContinue`] runs once its deps resolve. The three shapes are the
/// closed set of "what happens on wake":
/// - `Finish` consumes the resolved dep values and returns another [`Outcome`] (Combine).
/// - `Replay` re-runs the parked dispatch decide (the `ParkSelf` shape — its payload becomes a
///   resume closure once `DispatchState` dissolves).
/// - `Forward` makes the slot *be* a single producer's value (the bare-name `Lift` forward).
pub(in crate::machine::execute) enum Continuation<'run> {
    Finish(DispatchCombineFinish<'run>),
    Replay(DispatchState<'run>),
    Forward(NodeId),
}

/// A dependency a [`Outcome::ParkThenContinue`] declares. `Dispatch`/`*Lit` are fresh sub-slots
/// the harness submits (and owns); `Existing` is a pre-existing producer the decide phase found
/// that the slot merely parks on. Deps resolve in declaration order, so a finish reads
/// `results[k]` for the k-th dep.
pub(in crate::machine::execute) enum DispatchDep<'run> {
    Dispatch(KExpression<'run>),
    ListLit(Vec<ExpressionPart<'run>>),
    DictLit(Vec<(ExpressionPart<'run>, ExpressionPart<'run>)>),
    RecordLit(Vec<(String, ExpressionPart<'run>)>),
    Existing(NodeId),
}
