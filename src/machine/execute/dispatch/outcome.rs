//! The dispatch-side effect currency — the peer of
//! [`Action`](crate::machine::core::kfunction::action::Action) for the dispatcher.
//!
//! A dispatch *shape handler* decides against a read-only view of the scheduler and
//! **returns** its scheduler mutations as a [`DispatchOutcome`]; the harness
//! ([`super::harness`]) interprets that outcome and is the sole place that writes the
//! scheduler graph. This is the contract that lets `Scheduler` become the only
//! `SchedulerHandle` impl — a handler never reaches `&mut Scheduler` directly.
//!
//! The enum grows one variant per migrated handler (so an un-migrated handler that still
//! returns `NodeStep` produces no dead arm); the end state is a closed set the harness
//! interprets exhaustively.

use crate::machine::core::kfunction::KFunction;
use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::{NodeId, TraceFrame};

use super::super::nodes::{DispatchCombineFinish, NodeOutput};
use super::DispatchState;

/// What a decided dispatch slot wants the harness to do. Each variant is a pure data
/// description of a scheduler effect — no `&mut Scheduler` is captured.
pub(in crate::machine::execute) enum DispatchOutcome<'run> {
    /// Complete this slot with a value or error — the dispatcher reached a terminal with no
    /// graph write. The harness lowers it straight to [`NodeStep::Done`](super::super::nodes::NodeStep).
    Terminal(NodeOutput<'run>),
    /// Park the slot on `deps` as a [`NodeWork::DispatchCombine`](super::super::nodes::NodeWork):
    /// the harness submits each [`DispatchDep`], installs it as an owned edge, and re-enters
    /// `finish` with the resolved values. A dep error short-circuits frameless (or with
    /// `dep_error_frame`) before `finish` runs. The splice of resolved values into a
    /// `working_expr` lives entirely inside `finish` — the scheduler stays Future-unaware.
    Combine {
        deps: Vec<DispatchDep<'run>>,
        dep_error_frame: Option<TraceFrame>,
        finish: DispatchCombineFinish<'run>,
    },
    /// Park the slot on `producers` (pre-existing sibling producers it merely waits on, via
    /// `Notify` edges — never owned) and stash `state` so the slot re-decides on resume. The
    /// harness adds the park edges and replaces the slot with the parked `Dispatch`. The
    /// producers are the *to-wait* set the decide phase already filtered (cycle-free, deduped);
    /// the harness adds edges only — the cycle check is a decide-phase read.
    ParkSelf {
        producers: Vec<NodeId>,
        state: DispatchState<'run>,
    },
    /// Run a resolved call — the head uniquely picked `picked` and every value part of
    /// `working_expr` is already `Future`/literal-resolved. The harness hands it to
    /// [`exec::invoke`](super::exec::invoke) (the execution sink that holds `&mut Scheduler`)
    /// and lowers the resulting body onto the slot. This is the dispatch→execution boundary:
    /// a decide names the call; the harness runs it.
    Invoke {
        picked: &'run KFunction<'run>,
        working_expr: KExpression<'run>,
    },
    /// Re-resolve dispatch against a now fully-spliced `working_expr` — the post-eager-subs
    /// continuation when no function was speculatively pre-picked. The harness re-enters
    /// [`KeywordedState::finish`](super::keyworded::KeywordedState::finish), which re-runs
    /// resolution on the resolved expr and routes the result (a revealed element-typed
    /// `Future` then surfaces as a slot-terminal `DispatchFailed`).
    Redispatch {
        working_expr: KExpression<'run>,
    },
    /// Park a bare-identifier slot on the single `producer` that will bind its name, then
    /// *become* that value via [`NodeWork::Lift`](super::super::nodes::NodeWork): the harness
    /// adds the `Notify` park edge and replaces the slot with a pending `Lift`. Distinct from
    /// `ParkSelf` (which re-decides on resume): a `Lift` slot adopts the producer's resolved
    /// value directly, so there is no dispatch state to carry.
    ParkLift {
        producer: NodeId,
    },
}

/// A dependency a [`DispatchOutcome::Combine`] declares. `Dispatch`/`*Lit` are fresh sub-slots
/// the harness submits (and owns); `Existing` is a pre-existing producer the decide phase found
/// (a pending `Reuse`) that the slot merely parks on. Deps resolve in declaration order, so a
/// finish reads `results[k]` for the k-th dep.
pub(in crate::machine::execute) enum DispatchDep<'run> {
    Dispatch(KExpression<'run>),
    ListLit(Vec<ExpressionPart<'run>>),
    DictLit(Vec<(ExpressionPart<'run>, ExpressionPart<'run>)>),
    RecordLit(Vec<(String, ExpressionPart<'run>)>),
    Existing(NodeId),
}
