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
/// output at the current node's slot — the normal path. `Replace(work)` is the tail-call path:
/// rewrite the current node's `work` and re-enqueue the same `idx` so it runs again with the
/// new work. Constant memory across tail-call sequences because no fresh slot is allocated.
enum NodeStep<'a> {
    Done(NodeOutput<'a>),
    Replace(NodeWork<'a>),
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
}

/// A dynamic DAG of dispatch and execution work. The parser submits `Dispatch` nodes for each
/// top-level expression; running a `Dispatch` may add child `Dispatch`/`Bind`/`Aggregate`
/// nodes, and a builtin body that holds `&mut dyn SchedulerHandle` can also add `Dispatch`
/// nodes (used by `if_then` for its lazy `value` and by `KFunction::invoke` for user-defined
/// bodies). The execute loop pops from a FIFO queue; a `Bind` whose subs forward through to a
/// not-yet-run node gets re-queued at the back. Cycles are statically prevented because every
/// new node's `NodeId` is strictly greater than every node it can reach, so the queue is
/// guaranteed to drain.
pub struct Scheduler<'a> {
    nodes: Vec<Option<Node<'a>>>,
    results: Vec<Option<NodeOutput<'a>>>,
    queue: VecDeque<usize>,
}

impl<'a> Scheduler<'a> {
    pub fn new() -> Self {
        Self { nodes: Vec::new(), results: Vec::new(), queue: VecDeque::new() }
    }

    pub fn len(&self) -> usize { self.nodes.len() }
    pub fn is_empty(&self) -> bool { self.nodes.is_empty() }

    /// Submit an unresolved expression for the scheduler to dispatch + execute. Returns the
    /// `NodeId` whose result the eventual dispatch will produce. The only public way to add
    /// work; everything else (`Bind`, `Aggregate`) is internal scaffolding spawned during a
    /// `Dispatch` node's run.
    pub fn add_dispatch(&mut self, expr: KExpression<'a>) -> NodeId {
        self.add(NodeWork::Dispatch(expr))
    }

    fn add(&mut self, work: NodeWork<'a>) -> NodeId {
        let id = NodeId(self.nodes.len());
        self.nodes.push(Some(Node { work }));
        self.results.push(None);
        self.queue.push_back(id.index());
        id
    }

    /// Drain the FIFO queue. A `Bind`/`Aggregate` whose subs forward through to a not-yet-
    /// resolved node gets re-queued at the back. A node whose work returns `NodeStep::Replace`
    /// (the tail-call path) gets its work rewritten and re-enqueued at the *front* so the same
    /// slot runs again with the new work — no new allocation. Returns each top-level node's
    /// final resolved `KObject` indexed by `NodeId`. Takes `&mut self` so callers (and tests)
    /// can inspect post-run state like `nodes.len()`.
    pub fn execute(&mut self, scope: &mut Scope<'a>) -> Result<Vec<&'a KObject<'a>>, String> {
        while let Some(idx) = self.queue.pop_front() {
            let node = self.nodes[idx]
                .take()
                .expect("scheduler must not revisit a completed node");
            let work = node.work;
            // Bind/Aggregate may need their dep results to be fully resolved (forward chain
            // ending in a `Value`). If any forward-chains to an unresolved slot, requeue.
            if let Some(deps) = work_deps(&work) {
                if !deps.iter().all(|d| self.is_result_ready(*d)) {
                    self.nodes[idx] = Some(Node { work });
                    self.queue.push_back(idx);
                    continue;
                }
            }
            let step = match work {
                NodeWork::Dispatch(expr) => self.run_dispatch(expr, scope)?,
                NodeWork::Bind { expr, subs } => self.run_bind(expr, subs, scope)?,
                NodeWork::Aggregate { elements } => NodeStep::Done(self.run_aggregate(elements)),
            };
            match step {
                NodeStep::Done(output) => self.results[idx] = Some(output),
                NodeStep::Replace(new_work) => {
                    self.nodes[idx] = Some(Node { work: new_work });
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
    /// `Replace(Dispatch(expr))` when the body was a tail call (the slot gets rewritten
    /// in place by the execute loop).
    fn run_dispatch(
        &mut self,
        expr: KExpression<'a>,
        scope: &mut Scope<'a>,
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
                let sub_id = self.add_dispatch(inner);
                subs.push((i, sub_id));
            }
            let parent = KExpression { parts };
            if subs.is_empty() {
                let future = scope.dispatch(parent)?;
                return Ok(self.invoke_to_step(future, scope));
            }
            let bind_id = self.add(NodeWork::Bind { expr: parent, subs });
            return Ok(NodeStep::Done(NodeOutput::Forward(bind_id)));
        }

        let mut new_parts = Vec::with_capacity(expr.parts.len());
        let mut subs: Vec<(usize, NodeId)> = Vec::new();
        for (i, part) in expr.parts.into_iter().enumerate() {
            match part {
                ExpressionPart::Expression(boxed) => {
                    let sub_id = self.add_dispatch(*boxed);
                    subs.push((i, sub_id));
                    // Placeholder — overwritten with `Future(result)` at Bind time.
                    new_parts.push(ExpressionPart::Identifier(String::new()));
                }
                ExpressionPart::ListLiteral(items) => {
                    let agg_id = self.schedule_list_literal(items);
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
        let bind_id = self.add(NodeWork::Bind { expr: new_expr, subs });
        Ok(NodeStep::Done(NodeOutput::Forward(bind_id)))
    }

    fn run_bind(
        &mut self,
        mut expr: KExpression<'a>,
        subs: Vec<(usize, NodeId)>,
        scope: &mut Scope<'a>,
    ) -> Result<NodeStep<'a>, String> {
        for (part_idx, dep_id) in subs {
            let value = self.read_result(dep_id);
            expr.parts[part_idx] = ExpressionPart::Future(value);
        }
        let future = scope.dispatch(expr)?;
        Ok(self.invoke_to_step(future, scope))
    }

    fn run_aggregate(&self, elements: Vec<AggregateElement<'a>>) -> NodeOutput<'a> {
        let items: Vec<KObject<'a>> = elements
            .into_iter()
            .map(|e| match e {
                AggregateElement::Static(obj) => obj,
                AggregateElement::Dep(dep) => self.read_result(dep).deep_clone(),
            })
            .collect();
        let leaked: &'a KObject<'a> = Box::leak(Box::new(KObject::List(items)));
        NodeOutput::Value(leaked)
    }

    fn schedule_list_literal(&mut self, items: Vec<ExpressionPart<'a>>) -> NodeId {
        let mut elements: Vec<AggregateElement<'a>> = Vec::with_capacity(items.len());
        for item in items {
            match item {
                ExpressionPart::Expression(boxed) => {
                    let sub_id = self.add_dispatch(*boxed);
                    elements.push(AggregateElement::Dep(sub_id));
                }
                ExpressionPart::ListLiteral(inner) => {
                    let nested_id = self.schedule_list_literal(inner);
                    elements.push(AggregateElement::Dep(nested_id));
                }
                other => elements.push(AggregateElement::Static(other.resolve())),
            }
        }
        self.add(NodeWork::Aggregate { elements })
    }

    /// Run a bound future's body and translate its `BodyResult` into a `NodeStep`. `Value`
    /// becomes `Done(Value)` — the slot stores the result. `Tail(expr)` becomes
    /// `Replace(Dispatch(expr))` — the execute loop rewrites the current slot's work and
    /// re-runs it, producing the tail-call slot reuse that keeps recursion at constant
    /// scheduler memory.
    fn invoke_to_step(&mut self, future: KFuture<'a>, scope: &mut Scope<'a>) -> NodeStep<'a> {
        match future.function.invoke(scope, self, future.bundle) {
            BodyResult::Value(v) => NodeStep::Done(NodeOutput::Value(v)),
            BodyResult::Tail(expr) => NodeStep::Replace(NodeWork::Dispatch(expr)),
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
    fn add_dispatch(&mut self, expr: KExpression<'a>) -> NodeId {
        Scheduler::add_dispatch(self, expr)
    }
}

impl<'a> KFunction<'a> {
    /// Run this function's body for an already-bound call. Builtins call straight through to
    /// their `fn` pointer with the scheduler handle. User-defined functions:
    ///   1. Substitute each parameter `Identifier` in the body with `Future(value)` for the
    ///      bound argument, recursing into nested sub-expressions and list literals.
    ///   2. Return the substituted body as `BodyResult::Tail` so the scheduler rewrites the
    ///      caller's own slot to a fresh `Dispatch` of the body — a tail call reuses the
    ///      caller's slot in place rather than spawning a new node and forwarding to it.
    ///      Recursive user-fns therefore run in constant scheduler memory.
    /// Substitution rather than a child scope is the first-cut tradeoff: it handles the
    /// common case (parameters referenced by name in the body) without the per-call scope
    /// allocation the leak-fix roadmap item will need to handle anyway. The cost is that
    /// shadowing a parameter inside the body — `(LET x = 99)` when `x` is a parameter —
    /// rewrites the LET's name slot to a `Future`, which then fails LET's `Identifier`-typed
    /// match. That failure is loud (dispatch error), not silent.
    pub fn invoke(
        &'a self,
        scope: &mut Scope<'a>,
        sched: &mut dyn SchedulerHandle<'a>,
        bundle: ArgumentBundle<'a>,
    ) -> BodyResult<'a> {
        match &self.body {
            Body::Builtin(f) => f(scope, sched, bundle),
            Body::UserDefined(expr) => {
                BodyResult::Tail(substitute_params(expr.clone(), &bundle))
            }
        }
    }
}

/// Replace every `Identifier(name)` in `expr` whose name is a key in `bundle.args` with a
/// `Future(value)` carrying that arg's value. Recurses into nested `Expression` and
/// `ListLiteral` parts so a body like `(PRINT (x))` substitutes the inner `(x)` correctly.
/// `Keyword`, `Literal`, and `Future` parts pass through unchanged. Each substituted value
/// is `Box::leak`'d, consistent with the rest of the codebase's program-lifetime allocation
/// model — the leak-fix roadmap item will replace these with arena-allocated values.
fn substitute_params<'a>(
    expr: KExpression<'a>,
    bundle: &ArgumentBundle<'a>,
) -> KExpression<'a> {
    KExpression {
        parts: expr.parts.into_iter().map(|p| substitute_part(p, bundle)).collect(),
    }
}

fn substitute_part<'a>(
    part: ExpressionPart<'a>,
    bundle: &ArgumentBundle<'a>,
) -> ExpressionPart<'a> {
    match part {
        ExpressionPart::Identifier(name) => match bundle.get(&name) {
            Some(value) => {
                let leaked: &'a KObject<'a> = Box::leak(Box::new(value.deep_clone()));
                ExpressionPart::Future(leaked)
            }
            None => ExpressionPart::Identifier(name),
        },
        ExpressionPart::Expression(boxed) => {
            ExpressionPart::Expression(Box::new(substitute_params(*boxed, bundle)))
        }
        ExpressionPart::ListLiteral(items) => ExpressionPart::ListLiteral(
            items.into_iter().map(|p| substitute_part(p, bundle)).collect(),
        ),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        let mut scope = default_scope();
        let mut sched = Scheduler::new();
        let id1 = sched.add_dispatch(let_expr("x", 1.0));
        let id2 = sched.add_dispatch(let_expr("y", 2.0));

        let results = sched.execute(&mut scope).unwrap();

        assert_eq!(results.len(), 2);
        assert!(matches!(results[id1.index()], KObject::Number(n) if *n == 1.0));
        assert!(matches!(results[id2.index()], KObject::Number(n) if *n == 2.0));
        assert!(scope.data.contains_key("x"));
        assert!(scope.data.contains_key("y"));
    }

    #[test]
    fn later_expression_sees_earlier_binding_via_lookup() {
        // `(x)` parses as a sub-Expression; the scheduler walks the second top-level
        // expression, spawns a sub-Dispatch for `(x)`, and the LET node above runs first
        // because its NodeId is smaller. Tests the in-order processing invariant.
        let mut scope = default_scope();
        let mut sched = Scheduler::new();
        sched.add_dispatch(let_expr("a", 10.0));

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
        sched.add_dispatch(lookup_a);

        sched.execute(&mut scope).unwrap();
        assert!(matches!(scope.data.get("b"), Some(KObject::Number(n)) if *n == 10.0));
    }

    #[test]
    fn tail_call_reuses_node_slot_in_place() {
        // `IF true THEN ("hi")` — when the predicate is true, `if_then` returns
        // `BodyResult::Tail(value_expr)`. The scheduler should rewrite the if_then's own
        // slot to a `Dispatch(value_expr)` and re-run, NOT spawn a fresh slot and forward.
        // Without TCO this would land at len() == 2 (one slot for if_then, one for the
        // value evaluation). With TCO, len() == 1 — the if_then's slot was reused.
        let mut scope = default_scope();
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
        let id = sched.add_dispatch(expr);

        let results = sched.execute(&mut scope).unwrap();

        assert!(matches!(results[id.index()], KObject::KString(s) if s == "hi"));
        assert_eq!(
            sched.len(),
            1,
            "tail-call slot reuse: the if_then's original slot should have been rewritten \
             to evaluate `(\"hi\")`, not allocate a new slot",
        );
    }
}
