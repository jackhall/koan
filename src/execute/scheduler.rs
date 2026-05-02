use std::collections::VecDeque;

use crate::dispatch::kfunction::{
    ArgumentBundle, Body, BodyResult, KFunction, NodeId, SchedulerHandle,
};
use crate::dispatch::kobject::KObject;
use crate::dispatch::scope::{KFuture, Scope};
use crate::parse::kexpression::{ExpressionPart, KExpression};

/// What a scheduler node will produce when its work runs. `Value` is computed inline; `Forward`
/// says "my result is whatever node `id` produces" — set when a `Dispatch` spawns a `Bind` for
/// its sub-expression deps. `read_result` follows `Forward` chains until it lands on a `Value`.
/// Cycles are statically prevented because every `NodeId` produced by `add_*` is strictly
/// greater than every `NodeId` it could forward to.
enum NodeOutput<'a> {
    Value(&'a KObject<'a>),
    Forward(NodeId),
}

/// What `run_dispatch`/`run_bind` tells the execute loop to do next. `Done(output)` stores the
/// output at the current node's slot — the normal path. `Replace { work, scope }` is the
/// tail-call path: rewrite the current node's `work` and re-enqueue the same `idx` so it runs
/// again with the new work. When `scope` is `Some`, also rebind the node's scope (used by
/// user-fn invocation to install the per-call child scope); `None` keeps the existing scope.
/// Constant memory across tail-call sequences because no fresh slot is allocated.
enum NodeStep<'a> {
    Done(NodeOutput<'a>),
    Replace { work: NodeWork<'a>, scope: Option<&'a Scope<'a>> },
}

/// What a scheduler node will run.
///
/// - `Dispatch(expr)` is the entry point: walk the expression's parts, spawn `Dispatch` nodes
///   for nested `Expression` (and `ListLiteral`) parts, and emit a `Bind` node depending on
///   them. If there's no nesting, dispatch + invoke happen inline and the result is stored
///   directly. Replaces the old "eager dispatch in `schedule_expr`" path.
/// - `Bind { expr, subs }` is the old `Pending`: splice each dep's resolved value into `parts`
///   as `Future(...)`, dispatch the resulting expression, invoke the bound future.
/// - `Aggregate { elements }` materializes a list literal once each `Dep` element resolves.
enum NodeWork<'a> {
    Dispatch(KExpression<'a>),
    Bind {
        expr: KExpression<'a>,
        subs: Vec<(usize, NodeId)>,
    },
    Aggregate {
        elements: Vec<AggregateElement<'a>>,
    },
}

/// One slot in an `Aggregate` node. `Static` is an already-resolved value; `Dep` defers to a
/// previously-scheduled node. The mix lets a list literal like `[1 (LET x = 5) z]` schedule
/// only the sub-expression and inline the other two.
enum AggregateElement<'a> {
    Static(KObject<'a>),
    Dep(NodeId),
}

struct Node<'a> {
    work: NodeWork<'a>,
    /// The scope this node executes against. Top-level nodes carry the run-root scope; nodes
    /// spawned during a body's evaluation inherit their spawning node's scope; a user-fn's
    /// tail-replace installs a per-call child scope here so the body's lookups resolve
    /// parameters by name.
    scope: &'a Scope<'a>,
}

/// A dynamic DAG of dispatch and execution work. The parser submits `Dispatch` nodes for each
/// top-level expression; running a `Dispatch` may add child `Dispatch`/`Bind`/`Aggregate`
/// nodes, and a builtin body that holds `&mut dyn SchedulerHandle` can also add `Dispatch`
/// nodes (used by `if_then` for its lazy `value` and by `KFunction::invoke` for user-defined
/// bodies). The execute loop pops from a FIFO queue; a `Bind` whose subs forward through to a
/// not-yet-run node gets re-queued at the back. Cycles are statically prevented because every
/// new node's `NodeId` is strictly greater than every node it can reach, so the queue is
/// guaranteed to drain.
///
/// Each node carries the scope it should run against (`Node::scope`). Sub-nodes spawned by a
/// running node default to the spawning node's scope; user-fn invocation installs a per-call
/// child scope via `NodeStep::Replace { scope: Some(child) }`.
pub struct Scheduler<'a> {
    nodes: Vec<Option<Node<'a>>>,
    results: Vec<Option<NodeOutput<'a>>>,
    queue: VecDeque<usize>,
    /// Set by the execute loop to the currently-running node's scope. Read by
    /// `SchedulerHandle::add_dispatch` so builtin-side `add_dispatch` calls inherit the
    /// running node's scope. `None` outside the execute loop.
    active_scope: Option<*const Scope<'a>>,
}

impl<'a> Scheduler<'a> {
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            results: Vec::new(),
            queue: VecDeque::new(),
            active_scope: None,
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

    fn add(&mut self, work: NodeWork<'a>, scope: &'a Scope<'a>) -> NodeId {
        let id = NodeId(self.nodes.len());
        self.nodes.push(Some(Node { work, scope }));
        self.results.push(None);
        self.queue.push_back(id.index());
        id
    }

    /// Drain the FIFO queue. A `Bind`/`Aggregate` whose subs forward through to a not-yet-
    /// resolved node gets re-queued at the back. A node whose work returns
    /// `NodeStep::Replace` (the tail-call path) gets its work rewritten and re-enqueued at
    /// the *front* so the same slot runs again with the new work — no new allocation.
    /// `Replace { scope: Some(s) }` also rebinds the slot's scope to `s`. Returns each
    /// top-level node's final resolved `KObject` indexed by `NodeId`. Takes `&mut self` so
    /// callers (and tests) can inspect post-run state like `nodes.len()`.
    pub fn execute(&mut self) -> Result<Vec<&'a KObject<'a>>, String> {
        while let Some(idx) = self.queue.pop_front() {
            let node = self.nodes[idx]
                .take()
                .expect("scheduler must not revisit a completed node");
            let scope = node.scope;
            let work = node.work;
            // Bind/Aggregate may need their dep results to be fully resolved (forward chain
            // ending in a `Value`). If any forward-chains to an unresolved slot, requeue.
            if let Some(deps) = work_deps(&work) {
                if !deps.iter().all(|d| self.is_result_ready(*d)) {
                    self.nodes[idx] = Some(Node { work, scope });
                    self.queue.push_back(idx);
                    continue;
                }
            }
            self.active_scope = Some(scope as *const _);
            let step = match work {
                NodeWork::Dispatch(expr) => self.run_dispatch(expr, scope)?,
                NodeWork::Bind { expr, subs } => self.run_bind(expr, subs, scope)?,
                NodeWork::Aggregate { elements } => NodeStep::Done(self.run_aggregate(elements, scope)),
            };
            self.active_scope = None;
            match step {
                NodeStep::Done(output) => self.results[idx] = Some(output),
                NodeStep::Replace { work: new_work, scope: new_scope } => {
                    let next_scope = new_scope.unwrap_or(scope);
                    self.nodes[idx] = Some(Node { work: new_work, scope: next_scope });
                    self.queue.push_front(idx);
                }
            }
        }
        let n = self.results.len();
        Ok((0..n).map(|i| self.read_result(NodeId(i))).collect())
    }

    /// True iff `id`'s `Forward` chain ends in a stored `Value`. Used by the execute loop to
    /// decide whether a `Bind`/`Aggregate` whose subs depend on `id` is safe to run yet.
    fn is_result_ready(&self, id: NodeId) -> bool {
        let mut cur = id;
        loop {
            match self.results.get(cur.index()).and_then(|o| o.as_ref()) {
                Some(NodeOutput::Value(_)) => return true,
                Some(NodeOutput::Forward(next)) => cur = *next,
                None => return false,
            }
        }
    }

    /// Walk an unresolved expression. If `lazy_candidate` matches, only schedule the
    /// eager-position `Expression` parts; the lazy positions ride through as `KExpression`
    /// data into a builtin slot typed `KExpression` (`if_then`, `FN`). Otherwise schedule
    /// every `Expression` (and `ListLiteral`) part as a sub-dispatch / aggregate dep.
    /// Returns a `NodeStep`: `Done(Value)` for an inline-dispatched body that produced a
    /// value, `Done(Forward(bind_id))` when it spawned a `Bind` to wait on subs, or
    /// `Replace { work: Dispatch(expr), .. }` when the body was a tail call (the slot gets
    /// rewritten in place by the execute loop).
    fn run_dispatch(
        &mut self,
        expr: KExpression<'a>,
        scope: &'a Scope<'a>,
    ) -> Result<NodeStep<'a>, String> {
        if let Some(eager_indices) = scope.lazy_candidate(&expr) {
            let mut parts = expr.parts;
            let mut subs = Vec::with_capacity(eager_indices.len());
            for i in eager_indices {
                let inner = match std::mem::replace(
                    &mut parts[i],
                    ExpressionPart::Identifier(String::new()),
                ) {
                    ExpressionPart::Expression(boxed) => *boxed,
                    _ => unreachable!("lazy_candidate only flags Expression parts"),
                };
                let sub_id = self.add(NodeWork::Dispatch(inner), scope);
                subs.push((i, sub_id));
            }
            let parent = KExpression { parts };
            if subs.is_empty() {
                let future = scope.dispatch(parent)?;
                return Ok(self.invoke_to_step(future, scope));
            }
            let bind_id = self.add(NodeWork::Bind { expr: parent, subs }, scope);
            return Ok(NodeStep::Done(NodeOutput::Forward(bind_id)));
        }

        let mut new_parts = Vec::with_capacity(expr.parts.len());
        let mut subs: Vec<(usize, NodeId)> = Vec::new();
        for (i, part) in expr.parts.into_iter().enumerate() {
            match part {
                ExpressionPart::Expression(boxed) => {
                    let sub_id = self.add(NodeWork::Dispatch(*boxed), scope);
                    subs.push((i, sub_id));
                    // Placeholder — overwritten with `Future(result)` at Bind time.
                    new_parts.push(ExpressionPart::Identifier(String::new()));
                }
                ExpressionPart::ListLiteral(items) => {
                    let agg_id = self.schedule_list_literal(items, scope);
                    subs.push((i, agg_id));
                    new_parts.push(ExpressionPart::Identifier(String::new()));
                }
                other => new_parts.push(other),
            }
        }
        let new_expr = KExpression { parts: new_parts };
        if subs.is_empty() {
            let future = scope.dispatch(new_expr)?;
            return Ok(self.invoke_to_step(future, scope));
        }
        let bind_id = self.add(NodeWork::Bind { expr: new_expr, subs }, scope);
        Ok(NodeStep::Done(NodeOutput::Forward(bind_id)))
    }

    fn run_bind(
        &mut self,
        mut expr: KExpression<'a>,
        subs: Vec<(usize, NodeId)>,
        scope: &'a Scope<'a>,
    ) -> Result<NodeStep<'a>, String> {
        for (part_idx, dep_id) in subs {
            let value = self.read_result(dep_id);
            expr.parts[part_idx] = ExpressionPart::Future(value);
        }
        let future = scope.dispatch(expr)?;
        Ok(self.invoke_to_step(future, scope))
    }

    fn run_aggregate(&self, elements: Vec<AggregateElement<'a>>, scope: &'a Scope<'a>) -> NodeOutput<'a> {
        let items: Vec<KObject<'a>> = elements
            .into_iter()
            .map(|e| match e {
                AggregateElement::Static(obj) => obj,
                AggregateElement::Dep(dep) => self.read_result(dep).deep_clone(),
            })
            .collect();
        let arena = scope.arena.expect("Aggregate requires an arena-backed scope");
        let allocated: &'a KObject<'a> = arena.alloc_object(KObject::List(items));
        NodeOutput::Value(allocated)
    }

    fn schedule_list_literal(&mut self, items: Vec<ExpressionPart<'a>>, scope: &'a Scope<'a>) -> NodeId {
        let mut elements: Vec<AggregateElement<'a>> = Vec::with_capacity(items.len());
        for item in items {
            match item {
                ExpressionPart::Expression(boxed) => {
                    let sub_id = self.add(NodeWork::Dispatch(*boxed), scope);
                    elements.push(AggregateElement::Dep(sub_id));
                }
                ExpressionPart::ListLiteral(inner) => {
                    let nested_id = self.schedule_list_literal(inner, scope);
                    elements.push(AggregateElement::Dep(nested_id));
                }
                other => elements.push(AggregateElement::Static(other.resolve())),
            }
        }
        self.add(NodeWork::Aggregate { elements }, scope)
    }

    /// Run a bound future's body and translate its `BodyResult` into a `NodeStep`. `Value`
    /// becomes `Done(Value)` — the slot stores the result. `Tail { expr, scope }` becomes
    /// `Replace { work: Dispatch(expr), scope }` — the execute loop rewrites the current
    /// slot's work (and optionally rebinds scope) and re-runs it, producing the tail-call
    /// slot reuse that keeps recursion at constant scheduler memory.
    fn invoke_to_step(&mut self, future: KFuture<'a>, scope: &'a Scope<'a>) -> NodeStep<'a> {
        match future.function.invoke(scope, self, future.bundle) {
            BodyResult::Value(v) => NodeStep::Done(NodeOutput::Value(v)),
            BodyResult::Tail { expr, scope: new_scope } => NodeStep::Replace {
                work: NodeWork::Dispatch(expr),
                scope: new_scope,
            },
        }
    }

    fn read_result(&self, id: NodeId) -> &'a KObject<'a> {
        let mut cur = id;
        loop {
            match self.results[cur.index()]
                .as_ref()
                .expect("result must be ready by the time it's read")
            {
                NodeOutput::Value(v) => return v,
                NodeOutput::Forward(next) => cur = *next,
            }
        }
    }
}

impl<'a> Default for Scheduler<'a> {
    fn default() -> Self { Self::new() }
}

/// Dep `NodeId`s whose results a node needs to read before it can run, or `None` if the node
/// can run with no resolved deps. `Dispatch` itself has none — its job is to *spawn* deps; it
/// reads no results. `Bind` reads each `(_, dep)` in its subs; `Aggregate` reads each `Dep`
/// element.
fn work_deps<'a>(work: &NodeWork<'a>) -> Option<Vec<NodeId>> {
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
    }
}

impl<'a> SchedulerHandle<'a> for Scheduler<'a> {
    /// Schedule a fresh `Dispatch` node against the currently-active scope (the scope of the
    /// node whose body is calling this). Used by builtin bodies — currently no in-tree builtin
    /// reaches for it (TCO covers the prior `if_then` use), but the lever is preserved.
    fn add_dispatch(&mut self, expr: KExpression<'a>) -> NodeId {
        let scope_ptr = self.active_scope.expect("add_dispatch called outside execute loop");
        // SAFETY: `active_scope` is set immediately before invoking a node's body to a
        // `&'a Scope<'a>` that lives for the entire run, and cleared immediately after. The
        // pointer is only read while the body is running, so the original reference is still
        // valid.
        let scope: &'a Scope<'a> = unsafe { &*scope_ptr };
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
                let arena = scope
                    .arena
                    .expect("user-defined function call requires an arena-backed scope");
                let child = arena.alloc_scope(scope.child_for_call());
                for (name, rc) in bundle.args.iter() {
                    let cloned = rc.deep_clone();
                    let allocated: &'a KObject<'a> = arena.alloc_object(cloned);
                    child.add(name.clone(), allocated);
                }
                let substituted = substitute_params(expr.clone(), &bundle, arena);
                BodyResult::tail_with_scope(substituted, child)
            }
        }
    }
}

/// Replace every `Identifier(name)` in `expr` whose name is a key in `bundle.args` with a
/// `Future(value)` carrying that arg's arena-allocated value. Recurses into nested
/// `Expression` and `ListLiteral` parts so a body like `(PRINT (x))` substitutes the inner
/// `(x)` correctly. `Keyword`, `Literal`, and `Future` parts pass through unchanged. Each
/// substituted value is allocated via the arena (replacing the prior `Box::leak`-per-call
/// model from before the leak fix).
fn substitute_params<'a>(
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

        let results = sched.execute().unwrap();

        assert_eq!(results.len(), 2);
        assert!(matches!(results[id1.index()], KObject::Number(n) if *n == 1.0));
        assert!(matches!(results[id2.index()], KObject::Number(n) if *n == 2.0));
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
        // `IF true THEN ("hi")` — when the predicate is true, `if_then` returns
        // `BodyResult::Tail(value_expr)`. The scheduler should rewrite the if_then's own
        // slot to a `Dispatch(value_expr)` and re-run, NOT spawn a fresh slot and forward.
        // Without TCO this would land at len() == 2 (one slot for if_then, one for the
        // value evaluation). With TCO, len() == 1 — the if_then's slot was reused.
        let arena = RuntimeArena::new();
        let root = default_scope(&arena, Box::new(std::io::sink()));
        let mut sched = Scheduler::new();
        let value = KExpression {
            parts: vec![ExpressionPart::Literal(KLiteral::String("hi".into()))],
        };
        let expr = KExpression {
            parts: vec![
                ExpressionPart::Keyword("IF".into()),
                ExpressionPart::Literal(KLiteral::Boolean(true)),
                ExpressionPart::Keyword("THEN".into()),
                ExpressionPart::Expression(Box::new(value)),
            ],
        };
        let id = sched.add_dispatch(expr, root);

        let results = sched.execute().unwrap();

        assert!(matches!(results[id.index()], KObject::KString(s) if s == "hi"));
        assert_eq!(
            sched.len(),
            1,
            "tail-call slot reuse: the if_then's original slot should have been rewritten \
             to evaluate `(\"hi\")`, not allocate a new slot",
        );
    }
}
