use std::rc::Rc;

use crate::dispatch::runtime::CallArena;
use crate::dispatch::runtime::KError;
use crate::dispatch::kfunction::{KFunction, NodeId};
use crate::dispatch::values::KObject;
use crate::dispatch::runtime::Scope;
use crate::parse::kexpression::KExpression;

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
/// When a `Dispatch` has to defer to a `Bind`/`Aggregate` to wait on sub-deps, it spawns
/// the worker into a new slot and rewrites its own slot to `Lift { from: worker }` so the
/// result still surfaces under the original slot index without consumers chasing a chain.
/// The `lift_kobject` deep-clone in `execute`'s Done arm handles the case where the lifted
/// Value lives in a per-call arena that is about to drop.
pub(super) enum NodeWork<'a> {
    Dispatch(KExpression<'a>),
    Bind {
        expr: KExpression<'a>,
        subs: Vec<(usize, NodeId)>,
    },
    Aggregate {
        elements: Vec<AggregateElement<'a>>,
    },
    /// Resolved keys are converted to `KKey`; non-scalar keys produce `KErrorKind::ShapeError`.
    AggregateDict {
        entries: Vec<(AggregateElement<'a>, AggregateElement<'a>)>,
    },
    Lift {
        from: NodeId,
    },
}

/// One slot in an `Aggregate`, or one side of a pair in an `AggregateDict`. The split
/// lets literals inline already-resolved values and only schedule sub-expressions.
pub(super) enum AggregateElement<'a> {
    Static(KObject<'a>),
    Dep(NodeId),
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
pub(super) fn work_deps<'a>(work: &NodeWork<'a>) -> Option<Vec<NodeId>> {
    match work {
        NodeWork::Dispatch(_) => None,
        NodeWork::Bind { subs, .. } => Some(subs.iter().map(|(_, d)| *d).collect()),
        NodeWork::Aggregate { elements } => Some(
            elements
                .iter()
                .filter_map(|e| match e {
                    AggregateElement::Dep(d) => Some(*d),
                    AggregateElement::Static(_) => None,
                })
                .collect(),
        ),
        NodeWork::AggregateDict { entries } => Some(
            entries
                .iter()
                .flat_map(|(k, v)| [k, v])
                .filter_map(|e| match e {
                    AggregateElement::Dep(d) => Some(*d),
                    AggregateElement::Static(_) => None,
                })
                .collect(),
        ),
        NodeWork::Lift { from } => Some(vec![*from]),
    }
}

/// Same as `work_deps` but flattened to slot indices, with `Dispatch` yielding an empty
/// vec. Stored in a sidecar so reclamation can walk a node's owned sub-tree after its
/// `NodeWork` has been consumed.
pub(super) fn work_dep_indices<'a>(work: &NodeWork<'a>) -> Vec<usize> {
    match work_deps(work) {
        Some(ids) => ids.into_iter().map(|d| d.index()).collect(),
        None => Vec::new(),
    }
}
