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
use super::nodes::{work_dep_indices, work_deps, Node, NodeOutput, NodeStep, NodeWork};

/// A dynamic DAG of dispatch and execution work. The parser submits `Dispatch` nodes for each
/// top-level expression; running a `Dispatch` may add child `Dispatch`/`Bind`/`Aggregate`
/// nodes, and a builtin body that holds `&mut dyn SchedulerHandle` can also add `Dispatch`
/// nodes (used by `KFunction::invoke` for user-defined bodies). The execute loop pops from a
/// FIFO queue; a `Bind` whose subs forward through to a
/// not-yet-run node gets re-queued at the back. Cycles are statically prevented because every
/// new node's `NodeId` is strictly greater than every node it can reach, so the queue is
/// guaranteed to drain.
///
/// Each node carries the scope it should run against (`Node::scope`). Sub-nodes spawned by a
/// running node default to the spawning node's scope; user-fn invocation installs a per-call
/// child scope via `NodeStep::Replace { scope: Some(child) }`.
///
/// Implementation is split across sibling files: node types in [super::nodes], the
/// per-node-kind run methods in [super::run], lifted-value rebuilding in [super::lift],
/// and forward-chain finalization in [super::finalize]. This file holds the public API,
/// the execute loop, and the dispatch→execute bridge (`KFunction::invoke`).
pub struct Scheduler<'a> {
    pub(super) nodes: Vec<Option<Node<'a>>>,
    pub(super) results: Vec<Option<NodeOutput<'a>>>,
    pub(super) queue: VecDeque<usize>,
    /// Slots that returned `Done(Forward(_))` while owning a per-call frame and are now
    /// waiting for their forward chain to resolve. `finalize_ready_frames` only scans this
    /// vec rather than all `nodes`, keeping the per-iteration cost proportional to the
    /// number of in-flight user-fn calls (typically tiny) instead of total scheduler size.
    pub(super) frame_holding_slots: Vec<usize>,
    /// 1:1 with `nodes`: each entry is the list of sub-slot indices owned by that slot. A
    /// `Bind`'s entry holds its `subs` indices; an `Aggregate`/`AggregateDict`'s holds its
    /// `Dep` indices; a `Dispatch`'s entry is empty. Populated at `add()` time and consumed
    /// when the slot's run reads its deps (`run_bind`/`run_aggregate*` clear it after
    /// freeing each dep). The sidecar exists because `NodeWork` is moved out by `take()` in
    /// the execute loop, so the dep list isn't otherwise recoverable for transitive
    /// reclamation.
    pub(super) node_dependencies: Vec<Vec<usize>>,
    /// LIFO stack of slot indices whose `nodes`/`results`/`node_dependencies` entries are
    /// cleared and ready to be reused. `add()` pulls from here before extending the vecs,
    /// so transient-node reclamation lands as constant scheduler memory across tail-
    /// recursive bodies that spawn body-internal sub-`Dispatch`/`Bind` work each iteration.
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
            frame_holding_slots: Vec::new(),
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
        // Inherit the active slot's frame (if any) so sub-dispatch / sub-bind / sub-aggregate
        // slots spawned during a user-fn body's run keep that body's per-call arena alive
        // until they finalize. The Rc clone is what makes `current_frame()` available to
        // builtins like MATCH whose own frame's child scope's `outer` lives in the per-call
        // arena. Top-level adds (`add_dispatch` from outside `execute`) inherit `None`.
        let frame = self.active_frame.clone();
        let idx = match self.free_list.pop() {
            Some(i) => {
                // Reclaimed slot: overwrite the cleared entries in place. `nodes[i]` was
                // set to `None` by `free`; `results[i]` was cleared; `node_dependencies[i]`
                // was drained. The fresh `work`/`deps` populate them now.
                self.nodes[i] = Some(Node { work, scope, frame, function: None });
                self.results[i] = None;
                self.node_dependencies[i] = deps;
                i
            }
            None => {
                let i = self.nodes.len();
                self.nodes.push(Some(Node { work, scope, frame, function: None }));
                self.results.push(None);
                self.node_dependencies.push(deps);
                i
            }
        };
        self.queue.push_back(idx);
        NodeId(idx)
    }

    /// Drain the FIFO queue. A `Bind`/`Aggregate` whose subs forward through to a not-yet-
    /// resolved node gets re-queued at the back. A node whose work returns
    /// `NodeStep::Replace` (the tail-call path) gets its work rewritten and re-enqueued at
    /// the *front* so the same slot runs again with the new work — no new allocation.
    /// `Replace { frame: Some(f) }` also installs `f` on the slot, dropping the slot's
    /// previous frame; the new frame's `scope()` becomes the slot's scope and its `arena()`
    /// owns the per-call allocations.
    ///
    /// On `Done`: if the slot owned a frame, the body's return `Value` references memory
    /// inside the per-call arena that's about to drop. Lift the value into the captured
    /// scope's arena (= the per-call scope's `outer.arena`, which by lexical scoping is the
    /// FN's definition arena and outlives the call) by deep-cloning it. Forwards need no
    /// lift — the forwarded slot does its own lift on its own Done.
    ///
    /// Use `read(id)` to retrieve a top-level dispatch's result after `execute` returns.
    /// Internal Bind/Aggregate/sub-Dispatch slots' results may point into per-call arenas
    /// that finalization has freed; the public API keeps those internals out of reach.
    pub fn execute(&mut self) -> Result<(), KError> {
        while let Some(idx) = self.queue.pop_front() {
            let node = self.nodes[idx]
                .take()
                .expect("scheduler must not revisit a completed node");
            let scope = node.scope;
            let work = node.work;
            let prev_frame = node.frame;
            let prev_function = node.function;
            // Bind/Aggregate may need their dep results to be fully resolved (forward chain
            // ending in a `Value`). If any forward-chains to an unresolved slot, requeue.
            if let Some(deps) = work_deps(&work) {
                if !deps.iter().all(|d| self.is_result_ready(*d)) {
                    self.nodes[idx] = Some(Node {
                        work,
                        scope,
                        frame: prev_frame,
                        function: prev_function,
                    });
                    self.queue.push_back(idx);
                    continue;
                }
            }
            // Expose the slot's frame to builtins via `SchedulerHandle::current_frame` for
            // the duration of this slot's run. Restored on exit so nested re-entry through
            // the trait (none today, but the lever is preserved) sees the right ancestor.
            let prev_active = self.active_frame.take();
            self.active_frame = prev_frame.clone();
            let step = match work {
                NodeWork::Dispatch(expr) => self.run_dispatch(expr, scope)?,
                NodeWork::Bind { expr, subs } => self.run_bind(expr, subs, scope, idx)?,
                NodeWork::Aggregate { elements } => NodeStep::Done(self.run_aggregate(elements, scope, idx)),
                NodeWork::AggregateDict { entries } => {
                    NodeStep::Done(self.run_aggregate_dict(entries, scope, idx))
                }
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
                                    continue;
                                }
                            }
                            let lifted = dest.alloc_object(lifted_obj);
                            self.results[idx] = Some(NodeOutput::Value(lifted));
                            // `frame` drops here. If the lifted value cloned an Rc, the
                            // arena lives on; otherwise this is the last reference and
                            // the per-call arena frees.
                        }
                        (NodeOutput::Forward(target), Some(frame)) => {
                            // Body forwarded into sub-slots; keep the frame alive until
                            // the chain resolves. `finalize_ready_frames` then promotes
                            // the terminal value and drops the frame. Slot is out of the
                            // queue so `work` is a stub.
                            self.results[idx] = Some(NodeOutput::Forward(target));
                            self.nodes[idx] = Some(Node {
                                work: NodeWork::Dispatch(KExpression { parts: Vec::new() }),
                                scope,
                                frame: Some(frame),
                                function: prev_function.clone(),
                            });
                            self.frame_holding_slots.push(idx);
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
                        }
                        (other, None) => {
                            self.results[idx] = Some(other);
                        }
                    }
                }
                NodeStep::Replace { work: new_work, frame: new_frame, function: new_function } => {
                    // TCO: drop the slot's previous frame immediately. Lexical scoping
                    // means the new frame's child scope's `outer` is the captured scope,
                    // not the previous frame's, so this is safe.
                    drop(prev_frame);
                    let (next_scope, next_frame) = match new_frame {
                        Some(f) => {
                            // SAFETY: `f.scope()` borrows from `f`, but `f` is owned by the
                            // slot once installed. The `&'a` we hand to the next iteration
                            // is anchored to `self.nodes[idx]`'s storage, which lives until
                            // the slot drops or its frame is replaced again.
                            let s: &'a Scope<'a> = unsafe {
                                std::mem::transmute::<&Scope<'_>, &'a Scope<'a>>(f.scope())
                            };
                            (s, Some(f))
                        }
                        None => (scope, None),
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
                    self.queue.push_front(idx);
                }
            }
            // Finalize any frame-holding slots whose forward chain has now resolved.
            self.finalize_ready_frames();
        }
        Ok(())
    }

    /// Reclaim slot `idx` and the Bind/Aggregate sub-tree it owns. Walks `Forward` chain
    /// links and `node_dependencies` recursively, clearing `results` and pushing each freed
    /// index onto `free_list` for `add()` to reuse.
    ///
    /// Safe to call only on slots whose work has finished — `nodes[idx]` must be `None`.
    /// The defensive check at the top early-continues for any still-live slot (queued, in
    /// `frame_holding_slots`, or currently being processed) so a misfire doesn't corrupt
    /// active state.
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
            if let Some(NodeOutput::Forward(t)) = self.results[i].as_ref() {
                stack.push(t.index());
            }
            let deps = std::mem::take(&mut self.node_dependencies[i]);
            for d in deps {
                stack.push(d);
            }
            self.results[i] = None;
            self.free_list.push(i);
        }
    }

    /// True iff `id`'s `Forward` chain ends in a stored terminal output (`Value` or `Err`).
    /// Used by the execute loop to decide whether a `Bind`/`Aggregate` whose subs depend on
    /// `id` is safe to run yet. An errored sub is "ready" — the parent will short-circuit
    /// on it during `run_bind`/`run_aggregate` rather than dispatch.
    pub(super) fn is_result_ready(&self, id: NodeId) -> bool {
        let mut cur = id;
        loop {
            match self.results.get(cur.index()).and_then(|o| o.as_ref()) {
                Some(NodeOutput::Value(_)) => return true,
                Some(NodeOutput::Err(_)) => return true,
                Some(NodeOutput::Forward(next)) => cur = *next,
                None => return false,
            }
        }
    }

    /// Retrieve the resolved result for a top-level dispatch (a `NodeId` returned from
    /// `add_dispatch`). Walks `Forward` chains to a terminal `Value` (returned as `Ok`) or
    /// `Err` (returned as `Err`). Only safe to call on IDs returned by `add_dispatch` —
    /// internal Bind/Aggregate/sub-Dispatch slots' results may have been freed by
    /// `finalize_ready_frames` when their parent user-fn slot's per-call frame dropped, so
    /// reading them would be UAF.
    pub fn read_result(&self, id: NodeId) -> Result<&'a KObject<'a>, &KError> {
        let mut cur = id;
        loop {
            match self.results[cur.index()]
                .as_ref()
                .expect("result must be ready by the time it's read")
            {
                NodeOutput::Value(v) => return Ok(v),
                NodeOutput::Err(e) => return Err(e),
                NodeOutput::Forward(next) => cur = *next,
            }
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
                    child.add(name.clone(), allocated);
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
    fn free_reclaims_bind_subtree_and_forward_chain() {
        // Build a synthetic scheduler state representing:
        //   slot 0: parent Bind with subs [1]
        //   slot 1: sub-Dispatch whose result is Forward(2)
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
        // ownership/forward graph by hand.
        for i in [s0, s1, s2, s3] {
            sched.nodes[i] = None;
        }
        sched.results[s1] = Some(NodeOutput::Forward(NodeId(s2)));
        sched.results[s2] = Some(NodeOutput::Value(value));
        sched.results[s3] = Some(NodeOutput::Value(value));
        sched.node_dependencies[s0] = vec![s1];
        sched.node_dependencies[s2] = vec![s3];

        sched.free(s1);

        // s1, s2, s3 reclaimed; s0 untouched.
        assert!(sched.results[s1].is_none(), "s1 result cleared");
        assert!(sched.results[s2].is_none(), "s2 result cleared");
        assert!(sched.results[s3].is_none(), "s3 result cleared");
        assert!(sched.node_dependencies[s2].is_empty(), "s2 deps drained");
        assert_eq!(sched.node_dependencies[s0], vec![s1], "s0 deps untouched");
        let mut freed: Vec<usize> = sched.free_list.iter().copied().collect();
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
