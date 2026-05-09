use std::collections::VecDeque;
use std::rc::Rc;

use crate::dispatch::runtime::{CallArena, RuntimeArena};
use crate::dispatch::runtime::{Frame, KError, KErrorKind};
use crate::dispatch::kfunction::{
    ArgumentBundle, Body, BodyResult, KFunction, NodeId, SchedulerHandle,
};
use crate::dispatch::values::KObject;
use crate::dispatch::runtime::Scope;
use crate::parse::kexpression::{ExpressionPart, KExpression};

use super::lift::lift_kobject;
use super::nodes::{work_dep_indices, Node, NodeOutput, NodeStep, NodeWork};

/// Walk `scope` and its outer chain, looking for a function in `functions[expr.untyped_key()]`
/// whose `pre_run` extractor returns `Some(name)` for `expr`. The first such name wins.
/// Used by `Scheduler::add` to install the dispatch-time placeholder at *submission* time
/// — so a sibling submitted later can park on the placeholder even if the producer slot
/// hasn't yet been popped off the FIFO queue.
fn extract_pre_run_name<'a>(expr: &KExpression<'a>, scope: &'a Scope<'a>) -> Option<String> {
    let key = expr.untyped_key();
    let mut current: Option<&Scope<'a>> = Some(scope);
    while let Some(s) = current {
        let functions = s.functions.borrow();
        if let Some(bucket) = functions.get(&key) {
            for f in bucket.iter() {
                if let Some(extractor) = f.pre_run {
                    if let Some(name) = extractor(expr) {
                        return Some(name);
                    }
                }
            }
        }
        drop(functions);
        current = s.outer;
    }
    None
}

/// A dynamic DAG of dispatch and execution work. The parser submits `Dispatch` nodes for each
/// top-level expression; running a `Dispatch` may add child `Dispatch`/`Bind`/`Aggregate`
/// nodes, and a builtin body that holds `&mut dyn SchedulerHandle` can also add `Dispatch`
/// nodes (used by `KFunction::invoke` for user-defined bodies).
///
/// The execute loop drains two queues: an internal `ready_set` (populated by the notify-walk
/// when a producer's terminal write decrements every dependent's `pending_deps` to zero) and
/// a top-level FIFO `queue` (the submission order for top-level dispatches). The notify-walk
/// is the push/notify model: producers carry a `notify_list` of dependent slot indices, and a
/// terminal `Value`/`Err` write fans out to wake those dependents. Cycles are statically
/// prevented because every new node's `NodeId` is strictly greater than every node it can
/// depend on.
///
/// Each node carries the scope it should run against (`Node::scope`). Sub-nodes spawned by a
/// running node default to the spawning node's scope; user-fn invocation installs a per-call
/// child scope via `NodeStep::Replace { scope: Some(child) }`.
///
/// Implementation is split across sibling files: node types in [super::nodes], the
/// per-node-kind run methods in [super::run], lifted-value rebuilding in [super::lift].
/// This file holds the public API, the execute loop, the notify-walk, and the
/// dispatch→execute bridge (`KFunction::invoke`).
pub struct Scheduler<'a> {
    pub(super) nodes: Vec<Option<Node<'a>>>,
    pub(super) results: Vec<Option<NodeOutput<'a>>>,
    /// FIFO queue used only for top-level dispatches submitted via `add_dispatch`. Internal
    /// Bind/Aggregate slots arrive on `ready_set` instead — populated by the notify-walk when
    /// every dependency has produced its terminal output.
    pub(super) queue: VecDeque<usize>,
    /// LIFO/FIFO buffer of slot indices whose `pending_deps` reached zero (or which had no
    /// deps to begin with). Drained ahead of `queue` so internal work is consumed before
    /// the next top-level submission is dispatched.
    pub(super) ready_set: VecDeque<usize>,
    /// 1:1 with `nodes`: each entry is the list of *consumer* slot indices that depend on
    /// the entry's slot. When a producer slot writes a terminal `Value`/`Err`, the notify-
    /// walk drains this list, decrements each consumer's `pending_deps`, and pushes any
    /// consumer whose counter reached zero onto `ready_set`. Populated at `add()` time and
    /// cleared on `free()` so a reused slot doesn't inherit phantom edges.
    pub(super) notify_list: Vec<Vec<usize>>,
    /// 1:1 with `nodes`: count of dependencies whose terminal result hasn't yet been
    /// observed by this slot's notify-decrement. Set at `add()` time to the number of deps
    /// not already terminal. Decremented by the notify-walk; when it hits zero the slot is
    /// pushed onto `ready_set` (Bind/Aggregate are then unconditionally safe to run).
    pub(super) pending_deps: Vec<usize>,
    /// 1:1 with `nodes`: each entry is the list of sub-slot indices owned by that slot. A
    /// `Bind`'s entry holds its `subs` indices; an `Aggregate`/`AggregateDict`'s holds its
    /// `Dep` indices; a `Dispatch`'s entry is empty. Used by `free()` to walk a freed slot's
    /// owned sub-tree recursively. Populated at `add()` time; cleared by `run_bind` /
    /// `run_aggregate*` after they eagerly free their deps on the success path.
    ///
    /// Note: `notify_list` is the *forward* (producer -> consumer) edge set; this sidecar is
    /// the *backward* (parent -> owned-children) edge set, which the notify-walk doesn't
    /// need but `free()` does to reclaim transitive sub-trees.
    pub(super) node_dependencies: Vec<Vec<usize>>,
    /// LIFO stack of slot indices whose `nodes`/`results`/`notify_list`/`pending_deps`/
    /// `node_dependencies` entries are cleared and ready to be reused. `add()` pulls from
    /// here before extending the vecs, so transient-node reclamation lands as constant
    /// scheduler memory across tail-recursive bodies that spawn body-internal
    /// sub-`Dispatch`/`Bind` work each iteration.
    pub(super) free_list: Vec<usize>,
    /// Frame Rc of the slot currently being executed. Set by the execute loop right before
    /// calling `run_dispatch`/`run_bind` and cleared after; read by builtins via
    /// `SchedulerHandle::current_frame` so a frame-creating builtin (MATCH) can chain the
    /// caller's frame Rc onto its own new frame. Without that chain, a new frame whose child
    /// scope's `outer` lives in the caller's per-call arena dangles the moment the caller's
    /// frame is dropped on TCO replace.
    pub(super) active_frame: Option<Rc<CallArena>>,
}

impl<'a> Scheduler<'a> {
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            results: Vec::new(),
            queue: VecDeque::new(),
            ready_set: VecDeque::new(),
            notify_list: Vec::new(),
            pending_deps: Vec::new(),
            node_dependencies: Vec::new(),
            free_list: Vec::new(),
            active_frame: None,
        }
    }

    pub fn len(&self) -> usize { self.nodes.len() }
    pub fn is_empty(&self) -> bool { self.nodes.is_empty() }

    /// Submit an unresolved expression for the scheduler to dispatch + execute against
    /// `scope`. Returns the `NodeId` whose result the eventual dispatch will produce. The
    /// only public way to add work; everything else (`Bind`, `Aggregate`) is internal
    /// scaffolding spawned during a `Dispatch` node's run.
    pub fn add_dispatch(&mut self, expr: KExpression<'a>, scope: &'a Scope<'a>) -> NodeId {
        self.add(NodeWork::Dispatch(expr), scope)
    }

    pub(super) fn add(&mut self, work: NodeWork<'a>, scope: &'a Scope<'a>) -> NodeId {
        let deps = work_dep_indices(&work);
        let no_deps = deps.is_empty();
        // Pre-extract a binder name from a `Dispatch` work so the placeholder install can
        // fire at submission time (not at run-time). Installing at submission means a
        // *later* sibling submission that looks up `name` finds the placeholder
        // immediately — covers the FIFO-order forward-reference case where a consumer is
        // submitted before its producer. No-op for non-Dispatch work and for Dispatch
        // shapes whose picked function has no `pre_run`.
        let placeholder_install: Option<String> = match &work {
            NodeWork::Dispatch(expr) => extract_pre_run_name(expr, scope),
            _ => None,
        };
        // Inherit the active slot's frame (if any) so sub-dispatch / sub-bind / sub-aggregate
        // slots spawned during a user-fn body's run keep that body's per-call arena alive
        // until they finalize. The Rc clone is what makes `current_frame()` available to
        // builtins like MATCH whose own frame's child scope's `outer` lives in the per-call
        // arena. Top-level adds (`add_dispatch` from outside `execute`) inherit `None`.
        let frame = self.active_frame.clone();
        let idx = match self.free_list.pop() {
            Some(i) => {
                // Reclaimed slot: overwrite the cleared entries in place. `free()` cleared
                // `nodes[i]`/`results[i]`/`notify_list[i]`/`pending_deps[i]`/
                // `node_dependencies[i]` so the fresh values populate them now without
                // inheriting any phantom edges from the previous occupant.
                self.nodes[i] = Some(Node { work, scope, frame, function: None });
                self.results[i] = None;
                self.notify_list[i].clear();
                self.pending_deps[i] = 0;
                self.node_dependencies[i] = deps;
                i
            }
            None => {
                let i = self.nodes.len();
                self.nodes.push(Some(Node { work, scope, frame, function: None }));
                self.results.push(None);
                self.notify_list.push(Vec::new());
                self.pending_deps.push(0);
                self.node_dependencies.push(deps);
                i
            }
        };
        // Install the dispatch-time placeholder *before* enqueueing. Order matters: the
        // queued slot's `run_dispatch` will perform an idempotent re-install (which is a
        // no-op when `placeholders[name]` already maps to this `idx`). Failure to install
        // (e.g. `Rebind`) doesn't abort `add` here — the slot's `run_dispatch` will see the
        // collision later via `install_dispatch_placeholder` and surface the structured
        // error to the caller.
        if let Some(name) = placeholder_install {
            let _ = scope.install_placeholder(name, NodeId(idx));
        }
        let pending = self.register_slot_deps(idx);
        if pending == 0 {
            // No outstanding deps: enqueue immediately. Top-level dispatches (no deps, no
            // active frame) want submission-order on the FIFO `queue` so
            // `dispatches_independent_expressions_in_order` and the LET-then-lookup test
            // see top-level slots run before later top-level adds. Internal Bind/Aggregate
            // slots whose deps are already all terminal (the inline-leaf case) take the
            // internal-priority `ready_set`. The `active_frame.is_none() && deps.is_empty()`
            // condition is the conservative discriminator: a slot with no work and no
            // active frame is by construction a top-level submission.
            if self.active_frame.is_none() && no_deps {
                self.queue.push_back(idx);
            } else {
                self.ready_set.push_back(idx);
            }
        }
        // Otherwise the slot is parked, waiting on a producer's terminal write. The
        // `notify_consumers` walk pushes it onto `ready_set` when the last dep produces.
        NodeId(idx)
    }

    /// Register `idx` as a consumer on each not-yet-terminal dep recorded in
    /// `node_dependencies[idx]`, returning the number of edges installed (= the value
    /// `pending_deps[idx]` is set to). Producers already terminal (`is_result_ready` true)
    /// are skipped — a notify against them would never fire because the producer's
    /// notify-walk has already happened.
    ///
    /// Used by `add()` (initial slot creation) and by the Replace path in `execute` when a
    /// rewritten work introduces new deps (the Lift shim case, where a dispatch body that
    /// spawned a Bind rewrites the dispatch slot's work to `Lift { from: bind_id }`).
    fn register_slot_deps(&mut self, idx: usize) -> usize {
        let mut pending = 0usize;
        // Bound the borrow: read the dep indices into a local. The list is small (1 for
        // Lift, equal to the part-count for Bind) and built from `usize`, so the copy is
        // cheap; sharing the borrow with `notify_list[dep].push(idx)` would otherwise need
        // index-based juggling against the same `self`.
        let n = self.node_dependencies[idx].len();
        for i in 0..n {
            let dep = self.node_dependencies[idx][i];
            if self.is_result_ready(NodeId(dep)) {
                continue;
            }
            self.notify_list[dep].push(idx);
            pending += 1;
        }
        self.pending_deps[idx] = pending;
        pending
    }

    /// Drain pending work in two priority bands. `ready_set` (pushed to by `notify_consumers`
    /// when a producer's terminal write decrements every dependent's counter to zero) feeds
    /// internal Bind/Aggregate/sub-Dispatch slots first — once a sub-tree has produced its
    /// inputs we want to consume them before kicking off the next top-level dispatch. The
    /// FIFO `queue` feeds top-level dispatches (and Replace re-enqueues, see below) in
    /// submission order.
    ///
    /// A node whose work returns `NodeStep::Replace` (the tail-call path) gets its work
    /// rewritten and re-enqueued at the *front* of `ready_set` so the same slot runs again
    /// with the new work — no new allocation. `Replace { frame: Some(f) }` also installs
    /// `f` on the slot, dropping the slot's previous frame; the new frame's `scope()`
    /// becomes the slot's scope and its `arena()` owns the per-call allocations.
    ///
    /// On `Done`: if the slot owned a frame, the body's return `Value` references memory
    /// inside the per-call arena that's about to drop. Lift the value into the captured
    /// scope's arena (= the per-call scope's `outer.arena`, which by lexical scoping is the
    /// FN's definition arena and outlives the call) by deep-cloning it. When a body has to
    /// defer to a Bind to wait on sub-deps, `run_dispatch` issues a `Replace` that rewrites
    /// the slot's work to `Lift { from: bind_id }`. The Lift anchors the frame on the slot,
    /// parks on `notify_list[bind_id]`, and runs once the bind's terminal write wakes it.
    ///
    /// Use `read(id)` to retrieve a top-level dispatch's result after `execute` returns.
    /// Internal Bind/Aggregate/sub-Dispatch slots' results may point into per-call arenas
    /// that have been freed by their parent's eager-free path; the public API keeps those
    /// internals out of reach.
    pub fn execute(&mut self) -> Result<(), KError> {
        loop {
            let idx = match self.ready_set.pop_front() {
                Some(i) => i,
                None => match self.queue.pop_front() {
                    Some(i) => i,
                    None => break,
                },
            };
            let node = self.nodes[idx]
                .take()
                .expect("scheduler must not revisit a completed node");
            let scope = node.scope;
            let work = node.work;
            let prev_frame = node.frame;
            let prev_function = node.function;
            // Expose the slot's frame to builtins via `SchedulerHandle::current_frame` for
            // the duration of this slot's run. Restored on exit so nested re-entry through
            // the trait (none today, but the lever is preserved) sees the right ancestor.
            let prev_active = self.active_frame.take();
            self.active_frame = prev_frame.clone();
            let step = match work {
                NodeWork::Dispatch(expr) => self.run_dispatch(expr, scope, idx)?,
                NodeWork::Bind { expr, subs } => self.run_bind(expr, subs, scope, idx)?,
                NodeWork::Aggregate { elements } => NodeStep::Done(self.run_aggregate(elements, scope, idx)),
                NodeWork::AggregateDict { entries } => {
                    NodeStep::Done(self.run_aggregate_dict(entries, scope, idx))
                }
                NodeWork::Lift { from } => NodeStep::Done(self.run_lift(from)),
            };
            self.active_frame = prev_active;
            // Drain pending re-entrant writes from the dispatch that just ran while `scope`
            // is still guaranteed live — match arms below may drop the frame `scope` is
            // anchored to. See design/memory-model.md § Re-entrant `Scope::add`.
            scope.drain_pending();
            match step {
                NodeStep::Done(output) => {
                    match (output, prev_frame) {
                        (NodeOutput::Value(v), Some(frame)) => {
                            // Body produced a Value — lift into the captured arena
                            // (= per-call scope's `outer` by lexical scoping). See
                            // design/memory-model.md for the Rc<CallArena> anchoring story.
                            let dest = scope
                                .outer
                                .expect("per-call scope must have an outer (its captured scope)")
                                .arena;
                            let lifted_obj = lift_kobject(v, &frame);
                            // Runtime return-type check against the function's declared
                            // `signature.return_type`. `Any` short-circuits; mismatch
                            // synthesizes a TypeMismatch with the function's frame.
                            if let Some(f) = prev_function {
                                let rt = &f.signature.return_type;
                                if !rt.matches_value(&lifted_obj) {
                                    let err = KError::new(KErrorKind::TypeMismatch {
                                        arg: "<return>".to_string(),
                                        expected: rt.name(),
                                        got: lifted_obj.ktype().name(),
                                    })
                                    .with_frame(Frame {
                                        function: f.summarize(),
                                        expression: f.summarize(),
                                    });
                                    self.results[idx] = Some(NodeOutput::Err(err));
                                    self.notify_consumers(idx);
                                    continue;
                                }
                            }
                            let lifted = dest.alloc_object(lifted_obj);
                            self.results[idx] = Some(NodeOutput::Value(lifted));
                            self.notify_consumers(idx);
                            // `frame` drops here. If the lifted value cloned an Rc, the
                            // arena lives on; otherwise this is the last reference and
                            // the per-call arena frees.
                        }
                        (NodeOutput::Err(e), Some(_frame)) => {
                            // User-fn body errored. Drop the frame (the body's per-call
                            // arena was about to drop on Done anyway). Append a Frame
                            // naming the user-fn so the trace points to which call this
                            // error happened inside.
                            let with_frame = match prev_function {
                                Some(f) => e.with_frame(Frame {
                                    function: f.summarize(),
                                    expression: f.summarize(),
                                }),
                                None => e,
                            };
                            self.results[idx] = Some(NodeOutput::Err(with_frame));
                            self.notify_consumers(idx);
                        }
                        (other, None) => {
                            // Terminal output stored without a per-call frame to drop. Notify
                            // any consumers waiting on this slot. `other` is unconditionally
                            // `Value` or `Err` — `NodeOutput` has no other variants, since
                            // Lift's Done copies an existing terminal rather than introducing
                            // a third kind.
                            self.results[idx] = Some(other);
                            self.notify_consumers(idx);
                        }
                    }
                }
                NodeStep::Replace { work: new_work, frame: new_frame, function: new_function } => {
                    let (next_scope, next_frame) = match new_frame {
                        Some(f) => {
                            // TCO with a fresh per-call frame (user-fn invoke or MATCH-style
                            // builtin frame creation): drop the slot's previous frame. Lexical
                            // scoping means the new frame's child scope's `outer` is the
                            // captured scope, not the previous frame's, so this is safe.
                            drop(prev_frame);
                            // SAFETY: `f.scope()` borrows from `f`, but `f` is owned by the
                            // slot once installed. The `&'a` we hand to the next iteration
                            // is anchored to `self.nodes[idx]`'s storage, which lives until
                            // the slot drops or its frame is replaced again.
                            let s: &'a Scope<'a> = unsafe {
                                std::mem::transmute::<&Scope<'_>, &'a Scope<'a>>(f.scope())
                            };
                            (s, Some(f))
                        }
                        None => {
                            // Continuation in the same call. Inherit the slot's previous
                            // frame so the Lift shim (and the same-frame `tail()` builtins:
                            // struct_value, tagged_union, call-by-name) keep the per-call
                            // arena alive until the rewritten work finalizes.
                            (scope, prev_frame)
                        }
                    };
                    // Inherit prev_function when the replacement doesn't supply its own —
                    // a Tail without a fresh frame is staying in the same call.
                    let next_function = new_function.or(prev_function);
                    self.nodes[idx] = Some(Node {
                        work: new_work,
                        scope: next_scope,
                        frame: next_frame,
                        function: next_function,
                    });
                    // The new work's deps were recorded in `node_dependencies[idx]` by the
                    // run method that issued the Replace (e.g., `run_dispatch::defer_to_lift`
                    // appended `bind_id`). Either ready-enqueue (zero pending) or park on
                    // `notify_list` (the Lift shim case waits for the bind's terminal to
                    // wake the slot). Internal-priority on the front of `ready_set` so a
                    // ready Replace runs before queued top-level dispatches.
                    let pending = self.register_slot_deps(idx);
                    if pending == 0 {
                        self.ready_set.push_front(idx);
                    }
                    // Otherwise the slot waits on its deps; notify_consumers will wake it.
                }
            }
        }
        Ok(())
    }

    /// Drain `notify_list[idx]` after a terminal write to `results[idx]`. Each consumer's
    /// `pending_deps` decrements; consumers whose counter reaches zero are pushed onto
    /// `ready_set`. The notify edges are forward (producer -> consumer): a producer doesn't
    /// know what its consumers' dispatch needs, only that it has a terminal result they're
    /// waiting on.
    ///
    /// Idempotent on a producer side: drained `notify_list[idx]` is replaced by an empty
    /// `Vec`, so a second call (impossible by the once-only-write invariant on terminal
    /// outputs, but defensive) is a no-op.
    pub(super) fn notify_consumers(&mut self, idx: usize) {
        let notifees = std::mem::take(&mut self.notify_list[idx]);
        for consumer in notifees {
            // Every consumer in `notifees` is parked (its slot is live with a non-zero
            // `pending_deps` count) — the `freed_slot_does_not_appear_in_other_notify_lists`
            // invariant guarantees freed slots are scrubbed from every producer's notify_list
            // before the producer drains. Consumer's `pending_deps` therefore goes from N>=1
            // to N-1; when it hits zero, the slot is ready to run.
            self.pending_deps[consumer] -= 1;
            if self.pending_deps[consumer] == 0 {
                self.ready_set.push_back(consumer);
            }
        }
    }

    /// Reclaim slot `idx` and the Bind/Aggregate sub-tree it owns. Walks
    /// `node_dependencies` recursively, clearing `results` and pushing each freed index
    /// onto `free_list` for `add()` to reuse.
    ///
    /// Safe to call only on slots whose work has finished — `nodes[idx]` must be `None`.
    /// The defensive check at the top early-continues for any still-live slot (queued or
    /// currently being processed) so a misfire doesn't corrupt active state.
    ///
    /// Idempotent: the (results-is-none && deps-empty) guard early-continues when the slot
    /// was already reclaimed by an earlier path. A freshly-reused slot pulled from the
    /// free-list has `deps` repopulated by `add()` and won't be mistaken for already-freed.
    ///
    /// References handed out by `read(dep_id)` (the `&'a KObject` spliced into a parent's
    /// `expr.parts` as `Future(value)`) survive `free` because the underlying `KObject`
    /// lives in an arena; clearing `results[idx]` only drops the `NodeOutput::Value` enum
    /// wrapper, not the value it points at.
    pub(super) fn free(&mut self, idx: usize) {
        let mut stack = vec![idx];
        while let Some(i) = stack.pop() {
            if self.nodes[i].is_some() { continue; }
            if self.results[i].is_none() && self.node_dependencies[i].is_empty() {
                continue;
            }
            let deps = std::mem::take(&mut self.node_dependencies[i]);
            for d in deps {
                stack.push(d);
            }
            self.results[i] = None;
            self.free_list.push(i);
        }
    }

    /// True iff slot `id` holds a terminal result (`Value` or `Err`). Used by the execute
    /// loop to decide whether a `Bind`/`Aggregate` whose subs depend on `id` is safe to run
    /// yet. An errored sub is "ready" — the parent will short-circuit on it during
    /// `run_bind`/`run_aggregate` rather than dispatch.
    pub(super) fn is_result_ready(&self, id: NodeId) -> bool {
        matches!(
            self.results.get(id.index()).and_then(|o| o.as_ref()),
            Some(NodeOutput::Value(_)) | Some(NodeOutput::Err(_))
        )
    }

    /// Retrieve the resolved result for a top-level dispatch (a `NodeId` returned from
    /// `add_dispatch`). Returns `Ok(value)` for a terminal `Value` or `Err(&KError)` for
    /// a propagated error. Only safe to call on IDs returned by `add_dispatch` — internal
    /// Bind/Aggregate/sub-Dispatch slots' results may have been freed by their parent's
    /// eager-free path (`run_bind`/`run_aggregate*`) once consumed, so reading them
    /// would be UAF.
    pub fn read_result(&self, id: NodeId) -> Result<&'a KObject<'a>, &KError> {
        match self.results[id.index()]
            .as_ref()
            .expect("result must be ready by the time it's read")
        {
            NodeOutput::Value(v) => Ok(v),
            NodeOutput::Err(e) => Err(e),
        }
    }

    /// Convenience wrapper for the value-only path: panics if the result is an `Err`.
    /// Most existing tests assume the program ran successfully and want the returned
    /// `KObject` directly; they can keep calling `read`. Tests asserting on errors use
    /// `read_result`.
    pub fn read(&self, id: NodeId) -> &'a KObject<'a> {
        match self.read_result(id) {
            Ok(v) => v,
            Err(e) => panic!("read called on errored node: {e}"),
        }
    }
}

impl<'a> Default for Scheduler<'a> {
    fn default() -> Self { Self::new() }
}

impl<'a> SchedulerHandle<'a> for Scheduler<'a> {
    /// Schedule a fresh `Dispatch` node against `scope`. Used by builtin bodies that want
    /// to spawn sub-work — currently no in-tree builtin reaches for it (TCO covers the
    /// cases where this would otherwise be needed), but the lever is preserved.
    fn add_dispatch(&mut self, expr: KExpression<'a>, scope: &'a Scope<'a>) -> NodeId {
        Scheduler::add(self, NodeWork::Dispatch(expr), scope)
    }

    /// Active slot's frame `Rc<CallArena>` (if any). Set by `execute` for the duration of
    /// each slot's `run_dispatch`/`run_bind` and cleared when control returns to the loop.
    /// Builtins like MATCH that build a new `CallArena` whose child scope's `outer` is the
    /// call site clone this Rc into the new frame so the call-site arena stays alive while
    /// the new frame is in use.
    fn current_frame(&self) -> Option<Rc<CallArena>> {
        self.active_frame.clone()
    }
}

impl<'a> KFunction<'a> {
    /// Run this function's body for an already-bound call. Builtins call straight through to
    /// their `fn` pointer with the scheduler handle. User-defined functions:
    ///   1. Allocate a per-call child `Scope` parented to `scope` (the call site) and bind
    ///      each parameter value into the child's `data`. Future closure work and any body
    ///      that uses parameters in Identifier-typed slots (e.g. `(LET y = x)` reading x by
    ///      name) will rely on these bindings.
    ///   2. Substitute every parameter `Identifier` in a clone of the body with
    ///      `Future(arena.alloc(value))` so the parameters match typed slots at dispatch
    ///      time (`(PRINT x)` needs `x` to surface as a `Future(KString)`, not an
    ///      `Identifier`). The cloned value is arena-allocated, freeing on run-end.
    ///   3. Return the substituted body as `BodyResult::tail_with_scope(body, child)` so the
    ///      scheduler rewrites the caller's own slot to a fresh `Dispatch(body)` against the
    ///      child scope — a tail call reuses the caller's slot in place. Recursive user-fns
    ///      therefore run in constant scheduler memory.
    ///
    /// The child scope and substitution are complementary: substitution covers parameter
    /// references in typed-slot positions, the child scope covers Identifier-slot lookups
    /// (`(x)` parens-wrapped) and is the substrate for future closure capture. The
    /// substitution carrying its own arena means we don't depend on the child scope's
    /// `data` map for parameter values inside typed-slot dispatch.
    pub fn invoke(
        &'a self,
        scope: &'a Scope<'a>,
        sched: &mut dyn SchedulerHandle<'a>,
        bundle: ArgumentBundle<'a>,
    ) -> BodyResult<'a> {
        match &self.body {
            Body::Builtin(f) => f(scope, sched, bundle),
            Body::UserDefined(expr) => {
                // Build a fresh per-call frame whose arena owns the per-call allocations
                // (child scope, parameter clones, substituted body's Future rewrites).
                // `outer` is the FN's captured definition scope (lexical scoping); for top-
                // level FNs that's run-root, which outlives every per-call frame, so the
                // chain Rc is `None`. Closure escapes whose captured scope lives in a per-
                // call arena are kept alive externally via the lifted `KFunction(&fn,
                // Some(Rc))` on the user-bound value; the closure-escape coverage tests
                // (`closure_escapes_outer_call_and_remains_invocable`) lean on that.
                let outer = self.captured_scope();
                let frame: Rc<CallArena> = CallArena::new(outer, None);
                // Re-borrow through raw pointers so the borrow ends before the `frame` move
                // below. SAFETY: heap-pinning makes `arena_ptr` and `scope_ptr` valid for
                // the box's life; allocations into the arena live until `frame` drops.
                let arena_ptr: *const RuntimeArena = frame.arena();
                let scope_ptr: *const Scope<'_> = frame.scope();
                let inner_arena: &'a RuntimeArena = unsafe { &*(arena_ptr as *const _) };
                let child: &'a Scope<'a> = unsafe { &*(scope_ptr as *const _) };
                for (name, rc) in bundle.args.iter() {
                    let cloned = rc.deep_clone();
                    let allocated = inner_arena.alloc_object(cloned);
                    // Fresh per-call child scope: each parameter name is bound exactly
                    // once. `bind_value`'s rebind check would only fire if the FN's
                    // signature elements somehow agreed on the same name twice (a
                    // signature parser invariant we already enforce upstream), so the
                    // `_` swallow is a safety net rather than a recoverable path.
                    let _ = child.bind_value(name.clone(), allocated);
                }
                let substituted = substitute_params(expr.clone(), &bundle, inner_arena);
                BodyResult::tail_with_frame(substituted, frame, self)
            }
        }
    }
}

/// Replace every `Identifier(name)` in `expr` whose name is a key in `bundle.args` with a
/// `Future(value)` carrying that arg's arena-allocated value. Recurses into nested
/// `Expression` and `ListLiteral` parts so a body like `(PRINT (x))` substitutes the inner
/// `(x)` correctly. `Keyword`, `Literal`, and `Future` parts pass through unchanged. Each
/// substituted value is allocated via the arena.
pub(crate) fn substitute_params<'a>(
    expr: KExpression<'a>,
    bundle: &ArgumentBundle<'a>,
    arena: &'a crate::dispatch::runtime::RuntimeArena,
) -> KExpression<'a> {
    KExpression {
        parts: expr
            .parts
            .into_iter()
            .map(|p| substitute_part(p, bundle, arena))
            .collect(),
    }
}

fn substitute_part<'a>(
    part: ExpressionPart<'a>,
    bundle: &ArgumentBundle<'a>,
    arena: &'a crate::dispatch::runtime::RuntimeArena,
) -> ExpressionPart<'a> {
    match part {
        ExpressionPart::Identifier(name) => match bundle.get(&name) {
            Some(value) => {
                let allocated: &'a KObject<'a> = arena.alloc_object(value.deep_clone());
                ExpressionPart::Future(allocated)
            }
            None => ExpressionPart::Identifier(name),
        },
        ExpressionPart::Expression(boxed) => {
            ExpressionPart::Expression(Box::new(substitute_params(*boxed, bundle, arena)))
        }
        ExpressionPart::ListLiteral(items) => ExpressionPart::ListLiteral(
            items
                .into_iter()
                .map(|p| substitute_part(p, bundle, arena))
                .collect(),
        ),
        ExpressionPart::DictLiteral(pairs) => ExpressionPart::DictLiteral(
            pairs
                .into_iter()
                .map(|(k, v)| {
                    (
                        substitute_part(k, bundle, arena),
                        substitute_part(v, bundle, arena),
                    )
                })
                .collect(),
        ),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::runtime::RuntimeArena;
    use crate::dispatch::builtins::default_scope;
    use crate::parse::kexpression::{ExpressionPart, KExpression, KLiteral};

    fn let_expr<'a>(name: &str, value: f64) -> KExpression<'a> {
        KExpression {
            parts: vec![
                ExpressionPart::Keyword("LET".into()),
                ExpressionPart::Identifier(name.into()),
                ExpressionPart::Keyword("=".into()),
                ExpressionPart::Literal(KLiteral::Number(value)),
            ],
        }
    }

    #[test]
    fn dispatches_independent_expressions_in_order() {
        let arena = RuntimeArena::new();
        let root = default_scope(&arena, Box::new(std::io::sink()));
        let mut sched = Scheduler::new();
        let id1 = sched.add_dispatch(let_expr("x", 1.0), root);
        let id2 = sched.add_dispatch(let_expr("y", 2.0), root);

        sched.execute().unwrap();

        assert!(matches!(sched.read(id1), KObject::Number(n) if *n == 1.0));
        assert!(matches!(sched.read(id2), KObject::Number(n) if *n == 2.0));
        let data = root.data.borrow();
        assert!(data.contains_key("x"));
        assert!(data.contains_key("y"));
    }

    #[test]
    fn later_expression_sees_earlier_binding_via_lookup() {
        // `(x)` parses as a sub-Expression; the scheduler walks the second top-level
        // expression, spawns a sub-Dispatch for `(x)`, and the LET node above runs first
        // because its NodeId is smaller. Tests the in-order processing invariant.
        let arena = RuntimeArena::new();
        let root = default_scope(&arena, Box::new(std::io::sink()));
        let mut sched = Scheduler::new();
        sched.add_dispatch(let_expr("a", 10.0), root);

        let lookup_a = KExpression {
            parts: vec![
                ExpressionPart::Keyword("LET".into()),
                ExpressionPart::Identifier("b".into()),
                ExpressionPart::Keyword("=".into()),
                ExpressionPart::Expression(Box::new(KExpression {
                    parts: vec![ExpressionPart::Identifier("a".into())],
                })),
            ],
        };
        sched.add_dispatch(lookup_a, root);

        sched.execute().unwrap();
        let data = root.data.borrow();
        assert!(matches!(data.get("b"), Some(KObject::Number(n)) if *n == 10.0));
    }

    #[test]
    fn free_reclaims_owned_subtree() {
        // Build a synthetic scheduler state representing:
        //   slot 0: parent Bind with subs [1]
        //   slot 1: dispatch slot owning bind 2 (Lift shim case: run_dispatch pushed
        //           bind_id onto node_dependencies[s1])
        //   slot 2: nested Bind with subs [3]; result Value
        //   slot 3: leaf Dispatch with Value
        // After `free(1)` (the typical run_bind eager-free case), slots 1, 2, 3 should
        // be reclaimed onto `free_list`; slot 0 stays untouched. A subsequent `add()`
        // pulls from `free_list` rather than extending the vec.
        let arena = RuntimeArena::new();
        let root = default_scope(&arena, Box::new(std::io::sink()));
        let mut sched = Scheduler::new();
        let value: &KObject = arena.alloc_object(KObject::Number(42.0));
        // Allocate four slots by adding placeholder Dispatches.
        let mk_dispatch = || NodeWork::Dispatch(KExpression { parts: Vec::new() });
        let s0 = sched.add(mk_dispatch(), root).index();
        let s1 = sched.add(mk_dispatch(), root).index();
        let s2 = sched.add(mk_dispatch(), root).index();
        let s3 = sched.add(mk_dispatch(), root).index();
        // Simulate post-run state: clear nodes (work consumed by `take()`), wire the
        // ownership graph by hand. The Lift shim's run_dispatch path tracks ownership
        // via `node_dependencies[idx].push(bind_id.index())`, so s1 owns s2.
        for i in [s0, s1, s2, s3] {
            sched.nodes[i] = None;
        }
        sched.results[s1] = Some(NodeOutput::Value(value));
        sched.results[s2] = Some(NodeOutput::Value(value));
        sched.results[s3] = Some(NodeOutput::Value(value));
        sched.node_dependencies[s0] = vec![s1];
        sched.node_dependencies[s1] = vec![s2];
        sched.node_dependencies[s2] = vec![s3];

        sched.free(s1);

        // s1, s2, s3 reclaimed; s0 untouched.
        assert!(sched.results[s1].is_none(), "s1 result cleared");
        assert!(sched.results[s2].is_none(), "s2 result cleared");
        assert!(sched.results[s3].is_none(), "s3 result cleared");
        assert!(sched.node_dependencies[s1].is_empty(), "s1 deps drained");
        assert!(sched.node_dependencies[s2].is_empty(), "s2 deps drained");
        assert_eq!(sched.node_dependencies[s0], vec![s1], "s0 deps untouched");
        let mut freed: Vec<usize> = sched.free_list.to_vec();
        freed.sort();
        assert_eq!(freed, vec![s1, s2, s3]);

        // Reuse: next `add()` pulls from free_list (LIFO; last-pushed reused first).
        let reused = sched.add(mk_dispatch(), root).index();
        assert!(sched.free_list.len() == 2, "one slot popped from free_list");
        assert!([s1, s2, s3].contains(&reused), "reused index came from free_list");
    }

    #[test]
    fn free_skips_live_slot_and_is_idempotent() {
        let arena = RuntimeArena::new();
        let root = default_scope(&arena, Box::new(std::io::sink()));
        let mut sched = Scheduler::new();
        let mk_dispatch = || NodeWork::Dispatch(KExpression { parts: Vec::new() });
        let s = sched.add(mk_dispatch(), root).index();
        // Live slot: nodes[s] = Some. free should be a no-op.
        sched.free(s);
        assert!(sched.nodes[s].is_some());
        assert!(sched.free_list.is_empty());

        // Now mark complete and free.
        sched.nodes[s] = None;
        let value: &KObject = arena.alloc_object(KObject::Number(1.0));
        sched.results[s] = Some(NodeOutput::Value(value));
        sched.free(s);
        assert_eq!(sched.free_list, vec![s]);
        // Idempotent: second free is a no-op (already-freed early-continue).
        sched.free(s);
        assert_eq!(sched.free_list, vec![s], "no duplicate free");
    }

    #[test]
    fn freed_slot_does_not_appear_in_other_notify_lists() {
        // Reclamation invariant: after `free(idx)`, `idx` must not appear in any other
        // slot's `notify_list`. The invariant holds by construction — the only way `idx`
        // gets installed in `notify_list[producer]` is via `register_deps` at slot-add
        // time, and the producer drains its outgoing notify list when it writes its
        // terminal. By the time `idx` is eligible for free (its work has run, so its
        // `pending_deps` reached zero), every producer has already drained `idx` from
        // its notify list.
        //
        // This test runs a body that exercises the typical fan-out shape: a BIND with
        // sub-Dispatch deps, followed by free(). Asserts the invariant directly on the
        // scheduler's internal state after `execute()` returns.
        //
        // If a future change breaks this invariant — e.g., adds a path that frees a
        // slot before the producer drained — a stale edge would survive and could
        // misfire onto a reused slot. The test is the canary; the fix would be to
        // scrub `idx` out of every `notify_list[*]` inside `free()`.
        let arena = RuntimeArena::new();
        let root = default_scope(&arena, Box::new(std::io::sink()));
        let mut sched = Scheduler::new();

        // Run a small program with sub-Dispatch fan-out to populate notify edges.
        let exprs = crate::parse::expression_tree::parse(
            "LET x = 1\n\
             LET y = 2\n\
             LET z = (LET a = 3)",
        )
        .expect("parse should succeed");
        for e in exprs {
            sched.add_dispatch(e, root);
        }
        sched.execute().expect("program should run");

        // After execute, every freed slot index must be absent from every other slot's
        // notify_list. A slot is "freed" iff its index appears in `free_list`.
        let freed: std::collections::HashSet<usize> =
            sched.free_list.iter().copied().collect();
        for (producer_idx, consumers) in sched.notify_list.iter().enumerate() {
            for &consumer in consumers {
                assert!(
                    !freed.contains(&consumer),
                    "stale notify edge: producer slot {producer_idx} still lists \
                     freed consumer slot {consumer} in its notify_list",
                );
            }
        }
    }

    #[test]
    fn tail_call_reuses_node_slot_in_place() {
        // `MATCH true WITH (true -> ("hi") false -> ("no"))` returns `BodyResult::Tail`
        // for the matched branch. The scheduler should rewrite MATCH's slot to a
        // `Dispatch` of the branch body and re-run in place, not spawn a fresh slot.
        // Expect `len() == 1` (slot reused) rather than `2`.
        let arena = RuntimeArena::new();
        let root = default_scope(&arena, Box::new(std::io::sink()));
        let mut sched = Scheduler::new();
        let exprs = crate::parse::expression_tree::parse(
            "MATCH true WITH (true -> (\"hi\") false -> (\"no\"))",
        )
        .expect("parse should succeed");
        assert_eq!(exprs.len(), 1);
        let id = sched.add_dispatch(exprs.into_iter().next().unwrap(), root);

        sched.execute().unwrap();

        assert!(matches!(sched.read(id), KObject::KString(s) if s == "hi"));
        assert_eq!(
            sched.len(),
            1,
            "tail-call slot reuse: the MATCH's original slot should have been rewritten \
             to evaluate the matched branch's body, not allocate a new slot",
        );
    }
}
