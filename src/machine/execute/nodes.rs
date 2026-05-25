use std::rc::Rc;

use crate::machine::model::KObject;
use crate::machine::{CallArena, CatchFinish, CombineFinish, KError, KFunction, NodeId, Scope};
use crate::machine::model::ast::KExpression;

/// Terminal output of a node's run. Once a slot's `results` entry holds either variant,
/// no further write to that slot occurs until it is freed and reused.
pub(super) enum NodeOutput<'a> {
    Value(&'a KObject<'a>),
    Err(KError),
}

/// Outcome of a node's run. `Replace` is the tail-call path: rewrite the slot's work and
/// re-enqueue the same index so it runs again with no fresh slot allocated, giving constant
/// memory across tail-call sequences. When `frame` is `Some`, its `scope()` becomes the
/// slot's scope and its `arena()` owns per-call allocations; `None` keeps the existing
/// frame and scope. `function`, when set, names the user-fn whose body the replacement is
/// entering — any error landing on this slot gets a `Frame` appended for the trace.
pub(super) enum NodeStep<'a> {
    Done(NodeOutput<'a>),
    Replace {
        work: NodeWork<'a>,
        frame: Option<Rc<CallArena>>,
        function: Option<&'a KFunction<'a>>,
    },
}

/// What a scheduler node will run.
///
/// `Lift` exists because the push/notify model assumes a single producer slot per result.
/// When a `Dispatch` defers to a `Bind`/`Combine` for sub-deps, it spawns the worker into
/// a new slot and rewrites its own slot to `Lift(Pending(worker))` so the result still
/// surfaces under the original slot index. The notify-walk stamps `Pending → Ready` with
/// the producer's terminal output at wake time.
///
/// `Combine` is the dual of `Bind`: a host-side N→1 combinator that waits on a fixed set
/// of dep slots and runs a host closure over their resolved values.
pub(super) enum NodeWork<'a> {
    Dispatch(KExpression<'a>),
    Bind {
        expr: KExpression<'a>,
        subs: Vec<(usize, NodeId)>,
    },
    /// `deps` layout is `[park_producers..., owned_subs...]`. `park_count` is the
    /// size of the park-producer prefix — those slots are sibling producers this
    /// Combine merely reads at finish-time and does NOT own. Only the
    /// `deps[park_count..]` suffix gets installed as `DepEdge::Owned` and
    /// cascade-freed at success; the prefix installs as `Notify` (park) edges.
    Combine {
        deps: Vec<NodeId>,
        park_count: usize,
        finish: CombineFinish<'a>,
    },
    /// Catching dual of a single-dep `Combine`: waits on `from` and hands its terminal
    /// (Value or Err) to `finish`. The catching variant exists because `Combine` short-
    /// circuits on dep-error before its finish runs; `Catch`'s finish always runs so the
    /// closure can decide to recover. Backs `TRY-WITH`.
    Catch {
        from: NodeId,
        finish: CatchFinish<'a>,
    },
    Lift(LiftState<'a>),
}

/// `Pending(from)` parks on `from`'s terminal; `Ready(output)` holds the stamped
/// producer terminal. The `Pending → Ready` transition is the sole responsibility
/// of `Scheduler::finalize`; a queued Lift in `Pending` indicates a wake misfire.
pub(super) enum LiftState<'a> {
    Pending(NodeId),
    Ready(NodeOutput<'a>),
}

pub(super) struct Node<'a> {
    pub(super) work: NodeWork<'a>,
    pub(super) scope: &'a Scope<'a>,
    /// `Some` only for user-fn body slots. The Rc drops on Done or Replace; the arena
    /// itself drops then only if no escaped closure still holds the captured scope.
    /// Lexical scoping (`KFunction::captured`) makes each per-call child's `outer` the
    /// FN's captured scope, so no frame holds references a successor frame at the same
    /// slot needs — TCO drop is immediate with no `prev` chain.
    pub(super) frame: Option<Rc<CallArena>>,
    /// Set in lockstep with `frame`. Read on Done to enforce the declared return type
    /// and to append a `Frame` to errors for the call-stack trace.
    ///
    /// TCO limitation: when A tail-calls B, this is rewritten to B, so the runtime
    /// return-type check only fires against the tail-most function. Sound only when
    /// intermediate frames' types agree — to be enforced statically by the future
    /// type-check pass.
    pub(super) function: Option<&'a KFunction<'a>>,
}

/// Owned `NodeId`s a node must read before running, or `None` if it has no
/// owned read-deps. `Dispatch` spawns rather than reads, so returns `None`. For
/// `Combine`, only the `deps[park_count..]` suffix is owned; the park-producer
/// prefix is installed separately as `Notify` edges by `Scheduler::add`.
pub(super) fn work_deps<'a>(work: &NodeWork<'a>) -> Option<Vec<NodeId>> {
    match work {
        NodeWork::Dispatch(_) => None,
        NodeWork::Bind { subs, .. } => Some(subs.iter().map(|(_, d)| *d).collect()),
        NodeWork::Combine { deps, park_count, .. } => Some(deps[*park_count..].to_vec()),
        NodeWork::Catch { from, .. } => Some(vec![*from]),
        // `Lift` is only installed via `NodeStep::Replace` with deps wired explicitly;
        // arms exist for total coverage and are exercised by tests below.
        NodeWork::Lift(LiftState::Pending(from)) => Some(vec![*from]),
        NodeWork::Lift(LiftState::Ready(_)) => None,
    }
}

/// Park-producer prefix for a `Combine` (sibling slots whose values it splices
/// but does not own). Empty for every other work shape. The caller installs
/// each entry as a `Notify` edge separately from the Owned-edge install path.
pub(super) fn work_park_producers<'a, 'b>(work: &'b NodeWork<'a>) -> &'b [NodeId] {
    match work {
        NodeWork::Combine { deps, park_count, .. } => &deps[..*park_count],
        _ => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::core::{KError, KErrorKind};

    #[test]
    fn work_deps_lift_pending_returns_from_node() {
        let work = NodeWork::Lift(LiftState::Pending(NodeId(7)));
        assert_eq!(work_deps(&work), Some(vec![NodeId(7)]));
    }

    #[test]
    fn work_deps_lift_ready_returns_none() {
        let work = NodeWork::Lift(LiftState::Ready(NodeOutput::Err(
            KError::new(KErrorKind::User("stamped".to_string())),
        )));
        assert!(work_deps(&work).is_none());
    }
}
