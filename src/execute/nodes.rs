use std::rc::Rc;

use crate::dispatch::runtime::CallArena;
use crate::dispatch::runtime::KError;
use crate::dispatch::kfunction::{KFunction, NodeId};
use crate::dispatch::values::KObject;
use crate::dispatch::runtime::Scope;
use crate::parse::kexpression::KExpression;

/// Terminal output a scheduler node produces when its work runs. `Value` is the computed
/// result; `Err` says the work errored and dependents short-circuit and propagate it
/// (with a frame appended for context). Both are terminal — once a slot's `results` entry
/// holds either, `notify_consumers` wakes any parked consumers and no further write to
/// that slot occurs (until the slot is freed and reused).
pub(super) enum NodeOutput<'a> {
    Value(&'a KObject<'a>),
    Err(KError),
}

/// What `run_dispatch`/`run_bind` tells the execute loop to do next. `Done(output)` stores the
/// output at the current node's slot — the normal path. `Replace { work, frame, function }`
/// is the tail-call path: rewrite the current node's `work` and re-enqueue the same `idx` so
/// it runs again with the new work. When `frame` is `Some`, install it on the slot (its
/// `scope()` becomes the slot's scope, its `arena()` owns the per-call allocations) — used
/// by user-fn invocation. `None` keeps the existing frame and scope. `function` is an
/// optional label used to append a `Frame` to any error that lands on this slot — set by
/// user-fn invocation so an error inside a user-fn body carries the function's name in the
/// trace; `None` for non-call replacements (deferred-eval continuations). Constant memory
/// across tail-call sequences because no fresh slot is allocated.
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
/// - `Dispatch(expr)` is the entry point: walk the expression's parts, spawn `Dispatch` nodes
///   for nested `Expression` (and `ListLiteral`) parts, and emit a `Bind` node depending on
///   them. If there's no nesting, dispatch + invoke happen inline and the result is stored
///   directly.
/// - `Bind { expr, subs }`: splice each dep's resolved value into `parts` as `Future(...)`,
///   dispatch the resulting expression, invoke the bound future.
/// - `Aggregate { elements }` materializes a list literal once each `Dep` element resolves.
/// - `Lift { from }` is the deferring-dispatch shim. When `run_dispatch` finds it has to
///   wait on sub-Dispatch deps, it spawns a `Bind` for the actual work and rewrites its
///   own slot's work to `Lift { from: bind_id }`. The Lift slot is parked on
///   `notify_list[bind_id]`; once the bind writes its terminal `bind_id`'s notify-walk
///   wakes the Lift, whose run copies `results[from]` into the Lift slot's own result so
///   consumers see the same terminal. The push/notify model relies on a single producer
///   slot per result — Lift exists so a deferring dispatch surfaces under its original
///   slot index without consumers having to chase a chain. A frame-holding Lift's Done arm
///   in `execute` performs the `lift_kobject` deep-clone so the Value escapes the per-call
///   arena into the captured outer arena before the frame Rc drops.
pub(super) enum NodeWork<'a> {
    Dispatch(KExpression<'a>),
    Bind {
        expr: KExpression<'a>,
        subs: Vec<(usize, NodeId)>,
    },
    Aggregate {
        elements: Vec<AggregateElement<'a>>,
    },
    /// Materializes a dict literal once each key/value `Dep` resolves. Mirrors `Aggregate`'s
    /// shape but holds pairs and converts each resolved key to a `KKey` (rejecting non-scalar
    /// keys with `KErrorKind::ShapeError`) before inserting into the runtime `HashMap`. Each
    /// pair side reuses `AggregateElement` so the same Static/Dep machinery applies to keys
    /// and values alike.
    AggregateDict {
        entries: Vec<(AggregateElement<'a>, AggregateElement<'a>)>,
    },
    /// Deferring-dispatch shim: when the slot runs, copy `results[from]` into the slot's
    /// own result so consumers of this slot see the same terminal. The slot is parked on
    /// `notify_list[from]` at install time; `from`'s terminal write wakes it. Used
    /// whenever a Dispatch has to defer to a Bind/Aggregate to wait on sub-deps — the
    /// dispatch's slot is rewritten to `Lift { from: bind_id }` so its result surfaces
    /// under the original slot index rather than the bind's.
    Lift {
        from: NodeId,
    },
}

/// One slot in an `Aggregate` node, or one side of one pair in an `AggregateDict` node.
/// `Static` is an already-resolved value; `Dep` defers to a previously-scheduled node. The
/// mix lets a list literal like `[1 (LET x = 5) z]` schedule only the sub-expression and
/// inline the other two; for dicts both sides of a pair use the same shape so a literal
/// like `{(get_k): 1 a: (get_v)}` can defer either side while inlining scalars.
pub(super) enum AggregateElement<'a> {
    Static(KObject<'a>),
    Dep(NodeId),
}

pub(super) struct Node<'a> {
    pub(super) work: NodeWork<'a>,
    /// The scope this node executes against. Top-level nodes carry the run-root scope; nodes
    /// spawned during a body's evaluation inherit their spawning node's scope; a user-fn's
    /// tail-replace installs a per-call child scope here so the body's lookups resolve
    /// parameters by name.
    pub(super) scope: &'a Scope<'a>,
    /// Per-call frame this slot holds. `Some` for user-fn body slots, `None` for top-level
    /// dispatch and sub-Dispatch/Bind/Aggregate slots. The Rc drops when the slot reaches
    /// Done or is replaced; the underlying arena drops at that point only if no other Rc
    /// (e.g., from a closure that captured this frame's scope and escaped) is held.
    /// Lexical scoping (`KFunction::captured`) means each per-call child's `outer` is the
    /// FN's captured scope (run-root for top-level FNs), so a frame holds no references
    /// that a successor frame at the same slot needs — drop on TCO replace is immediate,
    /// no `prev` chain.
    pub(super) frame: Option<Rc<CallArena>>,
    /// User-fn reference installed by a TCO `Replace` whose body is `UserDefined`. The slot
    /// reads it on Done for two purposes: (1) enforce the function's declared
    /// `signature.return_type` against the produced value (the runtime return-type check),
    /// and (2) on error, append a `Frame { function: f.summarize() }` to the resulting
    /// `KError` so the call-stack trace names which user-fn the error happened inside.
    /// `None` for builtin slots and for non-call replacements (deferred-eval continuations).
    /// Set in lockstep with `frame` (a per-call frame implies a user-fn entry).
    ///
    /// TCO note: when A tail-calls B, this field is rewritten to B at the `Replace` site.
    /// The runtime check therefore only enforces the *tail-most* function's return type —
    /// sound for "the value the user sees has the type the outermost FN promised" only when
    /// intermediate frames' types agree, which the future static pass will check at compile
    /// time. Documented limitation.
    pub(super) function: Option<&'a KFunction<'a>>,
}

/// Dep `NodeId`s whose results a node needs to read before it can run, or `None` if the node
/// can run with no resolved deps. `Dispatch` itself has none — its job is to *spawn* deps; it
/// reads no results. `Bind` reads each `(_, dep)` in its subs; `Aggregate` reads each `Dep`
/// element.
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

/// Same as `work_deps` but returns owned `usize` slot indices and an empty vec (rather than
/// `None`) for `Dispatch`. Used by `Scheduler::add` to populate the `node_dependencies`
/// sidecar at slot-creation time so reclamation can walk a Bind/Aggregate's owned sub-tree
/// after its `NodeWork` has been consumed by `execute()`.
pub(super) fn work_dep_indices<'a>(work: &NodeWork<'a>) -> Vec<usize> {
    match work_deps(work) {
        Some(ids) => ids.into_iter().map(|d| d.index()).collect(),
        None => Vec::new(),
    }
}
