use std::rc::Rc;

use crate::runtime::machine::model::KObject;
use crate::runtime::machine::{CallArena, CombineFinish, KError, KFunction, NodeId, Scope};
use crate::ast::KExpression;

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
/// When a `Dispatch` has to defer to a `Bind`/`Combine` to wait on sub-deps, it spawns
/// the worker into a new slot and rewrites its own slot to `Lift { from: worker }` so the
/// result still surfaces under the original slot index without consumers chasing a chain.
/// The `lift_kobject` deep-clone in `execute`'s Done arm handles the case where the lifted
/// Value lives in a per-call arena that is about to drop.
///
/// `Combine` is the dual of `Bind`: a host-side N→1 combinator that waits on a fixed set
/// of dep slots and then runs an arbitrary host closure over their resolved values. List
/// and dict literals plan into `Combine` with their construction logic in `finish`'s
/// capture; future MODULE/SIG bodies will reuse the same primitive.
pub(super) enum NodeWork<'a> {
    Dispatch(KExpression<'a>),
    Bind {
        expr: KExpression<'a>,
        subs: Vec<(usize, NodeId)>,
    },
    Combine {
        deps: Vec<NodeId>,
        finish: CombineFinish<'a>,
    },
    Lift {
        from: NodeId,
    },
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

/// `NodeId`s a node must read before running, or `None` if it has no read-deps.
/// `Dispatch` returns `None` because it only spawns; it never reads results.
///
/// `DepEdge` and the `work_owned_edges` builder it feeds live in
/// `scheduler/dep_graph.rs` alongside the tri-vector state they populate.
pub(super) fn work_deps<'a>(work: &NodeWork<'a>) -> Option<Vec<NodeId>> {
    match work {
        NodeWork::Dispatch(_) => None,
        NodeWork::Bind { subs, .. } => Some(subs.iter().map(|(_, d)| *d).collect()),
        NodeWork::Combine { deps, .. } => Some(deps.clone()),
        NodeWork::Lift { from } => Some(vec![*from]),
    }
}
