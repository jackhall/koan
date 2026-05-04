use std::collections::VecDeque;
use std::rc::Rc;

use crate::dispatch::arena::{CallArena, RuntimeArena};
use crate::dispatch::kerror::{Frame, KError, KErrorKind};
use crate::dispatch::kfunction::{
    ArgumentBundle, Body, BodyResult, KFunction, NodeId, SchedulerHandle,
};
use crate::dispatch::kobject::KObject;
use crate::dispatch::scope::Scope;
use crate::parse::kexpression::{ExpressionPart, KExpression};

use super::lift::lift_kobject;
use super::nodes::{work_deps, Node, NodeOutput, NodeStep, NodeWork};

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
}

impl<'a> Scheduler<'a> {
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            results: Vec::new(),
            queue: VecDeque::new(),
            frame_holding_slots: Vec::new(),
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
        let id = NodeId(self.nodes.len());
        self.nodes.push(Some(Node { work, scope, frame: None, function: None }));
        self.results.push(None);
        self.queue.push_back(id.index());
        id
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
            let step = match work {
                NodeWork::Dispatch(expr) => self.run_dispatch(expr, scope)?,
                NodeWork::Bind { expr, subs } => self.run_bind(expr, subs, scope)?,
                NodeWork::Aggregate { elements } => NodeStep::Done(self.run_aggregate(elements, scope)),
                NodeWork::AggregateDict { entries } => {
                    NodeStep::Done(self.run_aggregate_dict(entries, scope))
                }
            };
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
            // Drain `scope`'s pending re-entrant writes between dispatch nodes so the next
            // node's reads see them. See design/memory-model.md § Re-entrant `Scope::add`.
            scope.drain_pending();
            // Finalize any frame-holding slots whose forward chain has now resolved.
            self.finalize_ready_frames();
        }
        Ok(())
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
                // level FNs that's run-root.
                let outer = self.captured_scope();
                let frame: Rc<CallArena> = CallArena::new(outer);
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
    arena: &'a crate::dispatch::arena::RuntimeArena,
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
    arena: &'a crate::dispatch::arena::RuntimeArena,
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
    use crate::dispatch::arena::RuntimeArena;
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
