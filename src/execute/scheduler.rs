use std::collections::VecDeque;
use std::rc::Rc;

use crate::dispatch::runtime::{CallArena, RuntimeArena};
use crate::dispatch::runtime::{Frame, KError, KErrorKind};
use crate::dispatch::kfunction::{
    ArgumentBundle, Body, BodyResult, CombineFinish, KFunction, NodeId, SchedulerHandle,
};
use crate::dispatch::values::KObject;
use crate::dispatch::runtime::Scope;
use crate::parse::kexpression::{ExpressionPart, KExpression};

use super::lift::lift_kobject;
use super::nodes::{work_owned_edges, DepEdge, Node, NodeOutput, NodeStep, NodeWork};

/// Walk `scope` and its outer chain, looking for a function in `functions[expr.untyped_key()]`
/// whose `pre_run` extractor returns `Some(name)` for `expr`. The first such name wins.
/// Submission-time install lets a later sibling park on the placeholder before the producer
/// slot is popped from the FIFO.
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
/// top-level expression; running a `Dispatch` may add child `Dispatch`/`Bind`/`Combine`
/// nodes, and builtin bodies holding `&mut dyn SchedulerHandle` can also add `Dispatch` nodes.
///
/// The execute loop drains two queues: internal `ready_set` (populated by the notify-walk
/// when a producer's terminal write decrements every dependent's `pending_deps` to zero) and
/// the top-level FIFO `queue` (submission order for top-level dispatches). Cycles are
/// statically prevented because every new node's `NodeId` is strictly greater than every
/// node it can depend on.
///
/// Each node carries the scope it should run against (`Node::scope`). Sub-nodes default to
/// the spawning node's scope; user-fn invocation installs a per-call child scope via
/// `NodeStep::Replace`.
///
/// See design/execution-model.md and design/memory-model.md.
pub struct Scheduler<'a> {
    pub(super) nodes: Vec<Option<Node<'a>>>,
    pub(super) results: Vec<Option<NodeOutput<'a>>>,
    /// Top-level dispatches submitted via `add_dispatch`. Internal Bind/Combine slots
    /// arrive on `ready_set` instead.
    pub(super) queue: VecDeque<usize>,
    /// Drained ahead of `queue` so internal work is consumed before the next top-level
    /// submission is dispatched.
    pub(super) ready_set: VecDeque<usize>,
    /// 1:1 with `nodes`: forward edges (producer -> consumer slot indices). Cleared on
    /// `free()` so a reused slot doesn't inherit phantom edges.
    pub(super) notify_list: Vec<Vec<usize>>,
    /// 1:1 with `nodes`: count of deps whose terminal result hasn't yet been observed by
    /// this slot's notify-decrement. Reaches zero -> slot pushed onto `ready_set`.
    pub(super) pending_deps: Vec<usize>,
    /// 1:1 with `nodes`: backward edges (consumer -> producer slots), tagged by kind.
    /// `DepEdge::Owned` marks a sub-slot this slot is responsible for reclaiming
    /// (Bind subs, Combine deps, Lift's `from`); `DepEdge::Notify` marks a sibling
    /// producer this slot only parked on for wake notification (§1 single-Identifier
    /// short-circuit, §8 replay-park). `notify_list` is the forward analogue;
    /// `free()` walks this sidecar but recurses only into `Owned` so park edges can
    /// never transit the reclaim walk into unrelated slots. Cleared by `run_bind` /
    /// `run_combine` after they eagerly free their deps on the success path.
    pub(super) dep_edges: Vec<Vec<DepEdge>>,
    /// Reclaimed slot indices. `add()` pulls from here before extending the vecs, so
    /// transient-node reclamation gives constant scheduler memory across tail-recursive
    /// bodies.
    pub(super) free_list: Vec<usize>,
    /// Frame Rc of the slot currently being executed. Read via `SchedulerHandle::current_frame`
    /// so frame-creating builtins (MATCH) can chain it onto their new frame; see
    /// [memory-model.md § Per-call-frame chaining](../../design/memory-model.md#per-call-frame-chaining-for-builtin-built-frames).
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
            dep_edges: Vec::new(),
            free_list: Vec::new(),
            active_frame: None,
        }
    }

    pub fn len(&self) -> usize { self.nodes.len() }
    pub fn is_empty(&self) -> bool { self.nodes.is_empty() }

    /// Submit an unresolved expression for the scheduler to dispatch + execute against
    /// `scope`. The only public way to add work; `Bind`/`Combine` are internal scaffolding
    /// spawned during a `Dispatch` node's run.
    pub fn add_dispatch(&mut self, expr: KExpression<'a>, scope: &'a Scope<'a>) -> NodeId {
        self.add(NodeWork::Dispatch(expr), scope)
    }

    /// Schedule a `Combine` slot. See `SchedulerHandle::add_combine`.
    pub fn add_combine(
        &mut self,
        deps: Vec<NodeId>,
        scope: &'a Scope<'a>,
        finish: CombineFinish<'a>,
    ) -> NodeId {
        self.add(NodeWork::Combine { deps, finish }, scope)
    }

    pub(super) fn add(&mut self, work: NodeWork<'a>, scope: &'a Scope<'a>) -> NodeId {
        let owned_edges = work_owned_edges(&work);
        let no_deps = owned_edges.is_empty();
        // Submission-time install lets a later sibling park on the placeholder before the
        // producer slot is popped from the FIFO. No-op for non-Dispatch work and for
        // Dispatch shapes whose picked function has no `pre_run`.
        let placeholder_install: Option<String> = match &work {
            NodeWork::Dispatch(expr) => extract_pre_run_name(expr, scope),
            _ => None,
        };
        // Inherit the active slot's frame so sub-slots spawned during a user-fn body's run
        // keep that body's per-call arena alive until they finalize.
        let frame = self.active_frame.clone();
        let idx = match self.free_list.pop() {
            Some(i) => {
                self.nodes[i] = Some(Node { work, scope, frame, function: None });
                self.results[i] = None;
                self.notify_list[i].clear();
                self.pending_deps[i] = 0;
                self.dep_edges[i] = owned_edges;
                i
            }
            None => {
                let i = self.nodes.len();
                self.nodes.push(Some(Node { work, scope, frame, function: None }));
                self.results.push(None);
                self.notify_list.push(Vec::new());
                self.pending_deps.push(0);
                self.dep_edges.push(owned_edges);
                i
            }
        };
        // Install before enqueueing: the queued slot's `run_dispatch` will idempotently
        // re-install. A failure here (e.g. `Rebind` collision) is surfaced later by
        // `install_dispatch_placeholder` rather than aborting `add`.
        if let Some(name) = placeholder_install {
            let _ = scope.install_placeholder(name, NodeId(idx));
        }
        let pending = self.register_slot_deps(idx);
        if pending == 0 {
            // Top-level dispatches (no active frame, no deps) take the FIFO `queue` for
            // submission-order. Internal slots whose deps are already terminal take the
            // internal-priority `ready_set`.
            if self.active_frame.is_none() && no_deps {
                self.queue.push_back(idx);
            } else {
                self.ready_set.push_back(idx);
            }
        }
        NodeId(idx)
    }

    /// Register `idx` as a consumer on each not-yet-terminal dep recorded in
    /// `dep_edges[idx]`, returning the count installed. Already-terminal producers
    /// are skipped — their notify-walk has already happened. Both `Owned` and `Notify`
    /// edges install a wake edge here: the kind distinction matters only at reclaim
    /// time (`free` recurses only into `Owned`).
    fn register_slot_deps(&mut self, idx: usize) -> usize {
        let mut pending = 0usize;
        let n = self.dep_edges[idx].len();
        for i in 0..n {
            let dep = self.dep_edges[idx][i].node_id().index();
            if self.is_result_ready(NodeId(dep)) {
                continue;
            }
            self.notify_list[dep].push(idx);
            pending += 1;
        }
        self.pending_deps[idx] = pending;
        pending
    }

    /// Drain pending work in two priority bands: `ready_set` (internal slots whose deps
    /// have all produced) feeds first, then the FIFO `queue` (top-level dispatches).
    ///
    /// `NodeStep::Replace` is the tail-call path: the slot's work is rewritten in place
    /// and re-enqueued at the front of `ready_set`. `Replace { frame: Some(f) }` installs
    /// `f` on the slot and drops the previous frame; the new frame's scope becomes the
    /// slot's scope and its arena owns the per-call allocations.
    ///
    /// On `Done` with a frame: the return `Value` references memory in the per-call arena
    /// that's about to drop, so it must be lifted into the captured scope's arena before
    /// the frame is released. See design/memory-model.md.
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
            // the duration of this slot's run; restored on exit.
            let prev_active = self.active_frame.take();
            self.active_frame = prev_frame.clone();
            let step = match work {
                NodeWork::Dispatch(expr) => self.run_dispatch(expr, scope, idx)?,
                NodeWork::Bind { expr, subs } => self.run_bind(expr, subs, scope, idx)?,
                NodeWork::Combine { deps, finish } => self.run_combine(deps, finish, scope, idx),
                NodeWork::Lift { from } => NodeStep::Done(self.run_lift(from)),
            };
            self.active_frame = prev_active;
            // Drain pending re-entrant writes while `scope` is still guaranteed live —
            // match arms below may drop the frame `scope` is anchored to. See
            // design/memory-model.md § Re-entrant `Scope::add`.
            scope.drain_pending();
            match step {
                NodeStep::Done(output) => {
                    match (output, prev_frame) {
                        (NodeOutput::Value(v), Some(frame)) => {
                            // Lift into the captured arena (per-call scope's `outer` by
                            // lexical scoping) before the frame drops. See
                            // design/memory-model.md.
                            let dest = scope
                                .outer
                                .expect("per-call scope must have an outer (its captured scope)")
                                .arena;
                            let lifted_obj = lift_kobject(v, &frame);
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
                            // `frame` drops here; if the lifted value cloned an Rc the
                            // arena lives on, otherwise it frees.
                        }
                        (NodeOutput::Err(e), Some(_frame)) => {
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
                            self.results[idx] = Some(other);
                            self.notify_consumers(idx);
                        }
                    }
                }
                NodeStep::Replace { work: new_work, frame: new_frame, function: new_function } => {
                    let (next_scope, next_frame) = match new_frame {
                        Some(f) => {
                            // Fresh per-call frame: drop the previous one. Lexical scoping
                            // means the new frame's child scope's `outer` is the captured
                            // scope, not the previous frame's.
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
                        None => (scope, prev_frame),
                    };
                    let next_function = new_function.or(prev_function);
                    self.nodes[idx] = Some(Node {
                        work: new_work,
                        scope: next_scope,
                        frame: next_frame,
                        function: next_function,
                    });
                    let pending = self.register_slot_deps(idx);
                    if pending == 0 {
                        self.ready_set.push_front(idx);
                    }
                }
            }
        }
        Ok(())
    }

    /// Drain `notify_list[idx]` after a terminal write to `results[idx]`, decrementing each
    /// consumer's `pending_deps` and pushing zero-counter consumers onto `ready_set`.
    ///
    /// Invariant: every consumer here is parked with a non-zero counter. Freed slots are
    /// scrubbed from every producer's `notify_list` before the producer drains (see the
    /// `freed_slot_does_not_appear_in_other_notify_lists` test).
    pub(super) fn notify_consumers(&mut self, idx: usize) {
        let notifees = std::mem::take(&mut self.notify_list[idx]);
        for consumer in notifees {
            self.pending_deps[consumer] -= 1;
            if self.pending_deps[consumer] == 0 {
                self.ready_set.push_back(consumer);
            }
        }
    }

    /// Reclaim slot `idx` and the sub-tree it owns. Walks `dep_edges` recursively but
    /// recurses only into `DepEdge::Owned` entries, clearing `results` and pushing each
    /// freed index onto `free_list`. `DepEdge::Notify` entries are dropped on the floor:
    /// they point at sibling producers this slot merely parked on, and reclaiming a
    /// consumer must not reach across a park edge into the producer's subtree.
    ///
    /// Idempotent and safe to call on a still-live slot: the guards early-continue when
    /// `nodes[idx]` is still `Some` or the slot was already reclaimed.
    ///
    /// `&'a KObject` references handed out by `read` survive `free` because the underlying
    /// value lives in an arena; clearing `results[idx]` only drops the enum wrapper.
    pub(super) fn free(&mut self, idx: usize) {
        let mut stack = vec![idx];
        while let Some(i) = stack.pop() {
            if self.nodes[i].is_some() { continue; }
            if self.results[i].is_none() && self.dep_edges[i].is_empty() {
                continue;
            }
            let edges = std::mem::take(&mut self.dep_edges[i]);
            for edge in edges {
                if let DepEdge::Owned(id) = edge {
                    stack.push(id.index());
                }
            }
            self.results[i] = None;
            self.free_list.push(i);
        }
    }

    /// True iff slot `id` holds a terminal result. An errored sub counts as ready — the
    /// parent short-circuits on it in `run_bind`/`run_combine`.
    pub(super) fn is_result_ready(&self, id: NodeId) -> bool {
        matches!(
            self.results.get(id.index()).and_then(|o| o.as_ref()),
            Some(NodeOutput::Value(_)) | Some(NodeOutput::Err(_))
        )
    }

    /// Retrieve the resolved result for a top-level dispatch. Only safe on IDs returned by
    /// `add_dispatch`; internal slots may have been eagerly freed by their parent.
    pub fn read_result(&self, id: NodeId) -> Result<&'a KObject<'a>, &KError> {
        match self.results[id.index()]
            .as_ref()
            .expect("result must be ready by the time it's read")
        {
            NodeOutput::Value(v) => Ok(v),
            NodeOutput::Err(e) => Err(e),
        }
    }

    /// Convenience wrapper for the value-only path: panics on `Err`.
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
    fn add_dispatch(&mut self, expr: KExpression<'a>, scope: &'a Scope<'a>) -> NodeId {
        Scheduler::add(self, NodeWork::Dispatch(expr), scope)
    }

    fn add_combine(
        &mut self,
        deps: Vec<NodeId>,
        scope: &'a Scope<'a>,
        finish: CombineFinish<'a>,
    ) -> NodeId {
        Scheduler::add_combine(self, deps, scope, finish)
    }

    /// Active slot's frame `Rc<CallArena>`, set by `execute` for the duration of each
    /// slot's run. Frame-creating builtins (MATCH) clone this Rc into the new frame so the
    /// call-site arena stays alive while the new frame is in use.
    fn current_frame(&self) -> Option<Rc<CallArena>> {
        self.active_frame.clone()
    }
}

impl<'a> KFunction<'a> {
    /// Run this function's body for an already-bound call. Builtins call straight through;
    /// user-defined functions allocate a per-call child scope, bind parameters into it,
    /// substitute parameter Identifiers in a body clone with `Future(value)`, and return a
    /// tail-call so the caller's slot is rewritten in place.
    ///
    /// The child scope and substitution are complementary: substitution covers parameter
    /// references in typed-slot positions (`(PRINT x)` needs `x` as a `Future(KString)`),
    /// the child scope covers Identifier-slot lookups (`(x)` parens-wrapped) and is the
    /// substrate for closure capture.
    pub fn invoke(
        &'a self,
        scope: &'a Scope<'a>,
        sched: &mut dyn SchedulerHandle<'a>,
        bundle: ArgumentBundle<'a>,
    ) -> BodyResult<'a> {
        match &self.body {
            Body::Builtin(f) => f(scope, sched, bundle),
            Body::UserDefined(expr) => {
                // Per-call frame whose arena owns the child scope, parameter clones, and
                // substituted-body allocations. `outer` is the FN's captured definition
                // scope (lexical scoping). Closure escapes whose captured scope lives in a
                // per-call arena are kept alive externally via the lifted
                // `KFunction(&fn, Some(Rc))` on the user-bound value.
                let outer = self.captured_scope();
                let frame: Rc<CallArena> = CallArena::new(outer, None);
                // SAFETY: heap-pinning makes `arena_ptr` and `scope_ptr` valid for the
                // box's life; allocations into the arena live until `frame` drops.
                let arena_ptr: *const RuntimeArena = frame.arena();
                let scope_ptr: *const Scope<'_> = frame.scope();
                let inner_arena: &'a RuntimeArena = unsafe { &*(arena_ptr as *const _) };
                let child: &'a Scope<'a> = unsafe { &*(scope_ptr as *const _) };
                for (name, rc) in bundle.args.iter() {
                    let cloned = rc.deep_clone();
                    let allocated = inner_arena.alloc_object(cloned);
                    // The signature parser enforces parameter-name uniqueness upstream, so
                    // `bind_value`'s rebind error here would indicate a signature-parser
                    // invariant break rather than a recoverable case.
                    let _ = child.bind_value(name.clone(), allocated);
                }
                let substituted = substitute_params(expr.clone(), &bundle, inner_arena);
                BodyResult::tail_with_frame(substituted, frame, self)
            }
        }
    }
}

/// Replace every `Identifier(name)` in `expr` whose name is in `bundle.args` with a
/// `Future(value)` allocated in `arena`. Recurses into nested `Expression`, `ListLiteral`,
/// and `DictLiteral` parts; other parts pass through unchanged.
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
        // The second top-level expression spawns a sub-Dispatch for `(x)`; the earlier
        // LET runs first because its NodeId is smaller. Guards in-order processing.
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
        // Synthetic state:
        //   slot 0: parent Bind with subs [1]
        //   slot 1: Lift-shim dispatch owning bind 2
        //   slot 2: nested Bind with subs [3], result Value
        //   slot 3: leaf Dispatch with Value
        // After `free(1)`: slots 1, 2, 3 reclaimed; slot 0 untouched.
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
        // Simulate post-run state and wire the ownership graph by hand.
        for i in [s0, s1, s2, s3] {
            sched.nodes[i] = None;
        }
        sched.results[s1] = Some(NodeOutput::Value(value));
        sched.results[s2] = Some(NodeOutput::Value(value));
        sched.results[s3] = Some(NodeOutput::Value(value));
        sched.dep_edges[s0] = vec![DepEdge::Owned(NodeId(s1))];
        sched.dep_edges[s1] = vec![DepEdge::Owned(NodeId(s2))];
        sched.dep_edges[s2] = vec![DepEdge::Owned(NodeId(s3))];

        sched.free(s1);

        // s1, s2, s3 reclaimed; s0 untouched.
        assert!(sched.results[s1].is_none(), "s1 result cleared");
        assert!(sched.results[s2].is_none(), "s2 result cleared");
        assert!(sched.results[s3].is_none(), "s3 result cleared");
        assert!(sched.dep_edges[s1].is_empty(), "s1 deps drained");
        assert!(sched.dep_edges[s2].is_empty(), "s2 deps drained");
        assert_eq!(sched.dep_edges[s0].len(), 1, "s0 edges untouched");
        assert!(
            matches!(sched.dep_edges[s0][0], DepEdge::Owned(id) if id.index() == s1),
            "s0 still owns s1",
        );
        let mut freed: Vec<usize> = sched.free_list.to_vec();
        freed.sort();
        assert_eq!(freed, vec![s1, s2, s3]);

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
        // Live slot: free should be a no-op.
        sched.free(s);
        assert!(sched.nodes[s].is_some());
        assert!(sched.free_list.is_empty());

        sched.nodes[s] = None;
        let value: &KObject = arena.alloc_object(KObject::Number(1.0));
        sched.results[s] = Some(NodeOutput::Value(value));
        sched.free(s);
        assert_eq!(sched.free_list, vec![s]);
        sched.free(s);
        assert_eq!(sched.free_list, vec![s], "no duplicate free");
    }

    #[test]
    fn free_does_not_recurse_through_notify_edges() {
        // Regression canary for the conflation bug fixed by `DepEdge`. Synthetic state:
        //   s_owner:   parent with dep_edges = [Owned(s_owned), Notify(s_sibling)]
        //   s_owned:   terminalized, owned by s_owner
        //   s_sibling: terminalized, parked-on by s_owner (must survive free of owner)
        // After `free(s_owner)`: only s_owner and s_owned land on `free_list`. The
        // sibling's `results` and `dep_edges` are untouched — the prior single-list
        // implementation would have reclaimed it as a transitive owned dep.
        let arena = RuntimeArena::new();
        let root = default_scope(&arena, Box::new(std::io::sink()));
        let mut sched = Scheduler::new();
        let value: &KObject = arena.alloc_object(KObject::Number(7.0));
        let mk_dispatch = || NodeWork::Dispatch(KExpression { parts: Vec::new() });
        let s_owner = sched.add(mk_dispatch(), root).index();
        let s_owned = sched.add(mk_dispatch(), root).index();
        let s_sibling = sched.add(mk_dispatch(), root).index();
        for i in [s_owner, s_owned, s_sibling] {
            sched.nodes[i] = None;
        }
        sched.results[s_owner] = Some(NodeOutput::Value(value));
        sched.results[s_owned] = Some(NodeOutput::Value(value));
        sched.results[s_sibling] = Some(NodeOutput::Value(value));
        // Give the sibling a non-empty edge list so the bug-shape would observably
        // walk into it: a self-loop would never be installed in the real scheduler,
        // but it lets us assert the walk stopped at the Notify edge by checking the
        // list is still intact after free.
        sched.dep_edges[s_owner] = vec![
            DepEdge::Owned(NodeId(s_owned)),
            DepEdge::Notify(NodeId(s_sibling)),
        ];
        sched.dep_edges[s_owned] = Vec::new();
        sched.dep_edges[s_sibling] = vec![DepEdge::Owned(NodeId(s_sibling))];

        sched.free(s_owner);

        let mut freed = sched.free_list.clone();
        freed.sort();
        let mut expected = vec![s_owner, s_owned];
        expected.sort();
        assert_eq!(freed, expected, "free must not recurse through Notify edges");
        assert!(
            sched.results[s_sibling].is_some(),
            "sibling's result must survive free of a slot that only parked on it",
        );
        assert_eq!(
            sched.dep_edges[s_sibling].len(),
            1,
            "sibling's dep_edges must survive (the free walk stopped at the Notify edge)",
        );
    }

    #[test]
    fn freed_slot_does_not_appear_in_other_notify_lists() {
        // Reclamation invariant: after `free(idx)`, `idx` must not appear in any other
        // slot's `notify_list`. Holds by construction — by the time `idx` is freed, its
        // pending_deps reached zero, which means every producer has already drained.
        // Canary against a future change that would free a slot before its producer
        // drained, leaving a stale edge to misfire onto a reused slot.
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
    fn combine_waits_on_deps_then_runs_finish() {
        // Direct exercise of `Combine`: two trivial dep slots that resolve to numbers,
        // a finish closure that concatenates their string renderings into a KString.
        // Pins the contract that Combine waits on every dep before invoking finish and
        // that finish-returned BodyResult::Value lands in the slot's result.
        use crate::dispatch::kfunction::{BodyResult, CombineFinish};
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let mut sched = Scheduler::new();
        let dep_a = sched.add_dispatch(let_expr("ca", 7.0), scope);
        let dep_b = sched.add_dispatch(let_expr("cb", 11.0), scope);
        let finish: CombineFinish = Box::new(|scope, _sched, results| {
            let a = match results[0] {
                KObject::Number(n) => *n,
                _ => return BodyResult::Err(crate::dispatch::runtime::KError::new(
                    crate::dispatch::runtime::KErrorKind::ShapeError("a not number".into()),
                )),
            };
            let b = match results[1] {
                KObject::Number(n) => *n,
                _ => return BodyResult::Err(crate::dispatch::runtime::KError::new(
                    crate::dispatch::runtime::KErrorKind::ShapeError("b not number".into()),
                )),
            };
            let allocated = scope.arena.alloc_object(KObject::KString(format!("{a}+{b}")));
            BodyResult::Value(allocated)
        });
        let combine_id = sched.add_combine(vec![dep_a, dep_b], scope, finish);
        sched.execute().unwrap();
        assert!(matches!(sched.read(combine_id), KObject::KString(s) if s == "7+11"));
    }

    #[test]
    fn combine_short_circuits_on_dep_error() {
        // Synthetic state: a Combine whose two deps already hold terminal results — one
        // Value, one Err. Pins the contract that finish does not run when any dep
        // errored, and that the propagated error carries a "<combine>" frame matching
        // run_bind's "<bind>" convention.
        use crate::dispatch::kfunction::{BodyResult, CombineFinish};
        use crate::dispatch::runtime::{KError, KErrorKind};
        use std::cell::Cell;
        use std::rc::Rc;
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let mut sched = Scheduler::new();

        // Allocate two placeholder Dispatch slots, drain the queue so add() doesn't
        // re-enqueue them at execute time, then overwrite their results directly
        // (mirrors the synthetic-state pattern used by `free_reclaims_owned_subtree`).
        let mk_dispatch = || NodeWork::Dispatch(KExpression { parts: Vec::new() });
        let dep_ok = sched.add(mk_dispatch(), scope);
        let dep_err = sched.add(mk_dispatch(), scope);
        sched.nodes[dep_ok.index()] = None;
        sched.nodes[dep_err.index()] = None;
        sched.queue.clear();
        sched.ready_set.clear();
        let value = arena.alloc_object(KObject::Number(99.0));
        sched.results[dep_ok.index()] = Some(NodeOutput::Value(value));
        sched.results[dep_err.index()] = Some(NodeOutput::Err(
            KError::new(KErrorKind::ShapeError("dep_err synthetic".into())),
        ));

        let invoked: Rc<Cell<bool>> = Rc::new(Cell::new(false));
        let invoked_clone = Rc::clone(&invoked);
        let finish: CombineFinish = Box::new(move |_scope, _sched, _results| {
            invoked_clone.set(true);
            BodyResult::Value(value)
        });
        let combine_id = sched.add_combine(vec![dep_ok, dep_err], scope, finish);
        sched.execute().unwrap();

        assert!(!invoked.get(), "finish must not run when a dep errored");
        let result = sched.read_result(combine_id);
        let err = match result {
            Err(e) => e.clone(),
            Ok(_) => panic!("combine should have errored"),
        };
        assert!(
            err.frames.iter().any(|f| f.function == "<combine>"),
            "propagated error should carry a <combine> frame, got {err}",
        );
    }

    #[test]
    fn defer_to_lifts_slot_terminal_off_combine_id() {
        // Round-trip for `BodyResult::DeferTo(id)`: a builtin body returns
        // `DeferTo(combine_id)`, the slot rewrites to `Lift { from: combine_id }`, the
        // Combine resolves to a value, and the builtin's slot ends up with the same
        // terminal as the Combine. Pins the binder-body wrap-up shape MODULE / SIG use.
        use crate::dispatch::builtins::{default_scope, register_builtin};
        use crate::dispatch::kfunction::{BodyResult, CombineFinish};
        use crate::dispatch::types::{ExpressionSignature, KType, SignatureElement};
        use crate::parse::kexpression::ExpressionPart;

        // Builtin "DEFERTEST": no args; schedules a Combine over zero deps whose finish
        // returns a known KString, then returns `BodyResult::DeferTo(combine_id)`.
        fn body<'a>(
            scope: &'a Scope<'a>,
            sched: &mut dyn crate::dispatch::kfunction::SchedulerHandle<'a>,
            _bundle: ArgumentBundle<'a>,
        ) -> BodyResult<'a> {
            let finish: CombineFinish<'a> = Box::new(|scope, _sched, _results| {
                let v = scope.arena.alloc_object(KObject::KString("from-combine".into()));
                BodyResult::Value(v)
            });
            let combine_id = sched.add_combine(Vec::new(), scope, finish);
            BodyResult::DeferTo(combine_id)
        }

        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        register_builtin(
            scope,
            "DEFERTEST",
            ExpressionSignature {
                return_type: KType::Str,
                elements: vec![SignatureElement::Keyword("DEFERTEST".into())],
            },
            body,
        );

        let mut sched = Scheduler::new();
        let id = sched.add_dispatch(
            KExpression { parts: vec![ExpressionPart::Keyword("DEFERTEST".into())] },
            scope,
        );
        sched.execute().unwrap();
        assert!(
            matches!(sched.read(id), KObject::KString(s) if s == "from-combine"),
            "DEFERTEST slot's terminal should match the Combine's terminal",
        );
    }

    #[test]
    fn tail_call_reuses_node_slot_in_place() {
        // MATCH returns `BodyResult::Tail`; the scheduler rewrites MATCH's slot to a
        // Dispatch of the matched branch body in place rather than spawning a fresh slot.
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
