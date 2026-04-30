use std::collections::VecDeque;

use crate::dispatch::kobject::KObject;
use crate::dispatch::scope::{KFuture, Scope};
use crate::parse::kexpression::{ExpressionPart, KExpression};

/// Stable handle to a node in a `Scheduler`'s DAG. Returned by `Scheduler::add`,
/// `add_with_deps`, and `add_pending`, and used to declare a later node's dependencies on
/// earlier ones.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct NodeId(usize);

impl NodeId {
    pub fn index(self) -> usize { self.0 }
}

/// What a scheduler node will run. `Bound` is a `KFuture` whose `ArgumentBundle` was already
/// produced at submission time. `Pending` is an unbound `KExpression` plus substitution edges
/// `(part_index, dep)`: at execute time the scheduler splices each dep's result into the
/// expression's parts, then dispatches and binds against the live `Scope` to obtain a future.
/// Pending lets a parent call wait on the runtime values of its sub-expressions.
enum NodeWork<'a> {
    Bound(KFuture<'a>),
    Pending {
        expr: KExpression<'a>,
        subs: Vec<(usize, NodeId)>,
    },
}

/// One vertex of the scheduler DAG: the work to perform plus the ids of nodes whose execution
/// must complete before this one runs.
struct Node<'a> {
    work: NodeWork<'a>,
    deps: Vec<NodeId>,
}

/// Holds a directed acyclic graph of deferred work and runs it in dependency order. Callers
/// register pre-bound futures via `add`/`add_with_deps`, or unbound expressions whose
/// arguments depend on other nodes' results via `add_pending`; each call returns a `NodeId`
/// that can be reused as a dependency for later additions. `execute` performs a Kahn-style
/// topological sort, materializes each pending expression once its deps have produced values,
/// invokes the function body against the supplied root `Scope`, and yields the produced
/// `KObject` references in submission order.
pub struct Scheduler<'a> {
    nodes: Vec<Node<'a>>,
}

impl<'a> Scheduler<'a> {
    pub fn new() -> Self {
        Self { nodes: Vec::new() }
    }

    /// Insert a future with no dependencies and return its `NodeId`.
    pub fn add(&mut self, future: KFuture<'a>) -> NodeId {
        self.add_with_deps(future, Vec::new())
    }

    /// Insert a future that must run after every `dep` has completed. Each `dep` must refer to a
    /// node already in the scheduler — `NodeId`s are only minted by add methods, so edges always
    /// point backwards in submission order and the graph is acyclic by construction.
    pub fn add_with_deps(&mut self, future: KFuture<'a>, deps: Vec<NodeId>) -> NodeId {
        let id = NodeId(self.nodes.len());
        Self::check_deps(id, &deps);
        self.nodes.push(Node { work: NodeWork::Bound(future), deps });
        id
    }

    /// Insert an unbound `KExpression` whose `(part_index, dep)` substitutions name the nodes
    /// whose results should be spliced in before dispatch. Deps are derived from `subs` and
    /// must refer to nodes already in the scheduler.
    pub fn add_pending(&mut self, expr: KExpression<'a>, subs: Vec<(usize, NodeId)>) -> NodeId {
        let id = NodeId(self.nodes.len());
        let mut deps: Vec<NodeId> = subs.iter().map(|(_, d)| *d).collect();
        deps.sort_by_key(|n| n.0);
        deps.dedup();
        Self::check_deps(id, &deps);
        self.nodes.push(Node { work: NodeWork::Pending { expr, subs }, deps });
        id
    }

    fn check_deps(id: NodeId, deps: &[NodeId]) {
        for dep in deps {
            assert!(
                dep.0 < id.0,
                "scheduler dependency NodeId({}) does not refer to an existing node",
                dep.0,
            );
        }
    }

    pub fn len(&self) -> usize { self.nodes.len() }
    pub fn is_empty(&self) -> bool { self.nodes.is_empty() }

    /// Topologically sort the DAG and run each node against `scope`. For `Pending` nodes,
    /// substitute each dep's already-computed result into the expression's parts before
    /// dispatching. Returns the produced `KObject`s indexed by `NodeId` (i.e. submission
    /// order), or an error string if a node fails to dispatch or the graph somehow contains a
    /// cycle.
    pub fn execute(self, scope: &mut Scope<'a>) -> Result<Vec<&'a KObject<'a>>, String> {
        let order = self.topo_order()?;
        let n = self.nodes.len();
        let mut nodes: Vec<Option<Node<'a>>> = self.nodes.into_iter().map(Some).collect();
        let mut results: Vec<Option<&'a KObject<'a>>> = vec![None; n];
        for idx in order {
            let node = nodes[idx].take().expect("topological order must not revisit a node");
            let value: &'a KObject<'a> = match node.work {
                NodeWork::Bound(future) => {
                    let body = future.function.body;
                    body(scope, future.bundle)
                }
                NodeWork::Pending { mut expr, subs } => {
                    for (part_idx, dep) in subs {
                        let dep_value = results[dep.0]
                            .expect("dependency must have produced a result before this node");
                        expr.parts[part_idx] = ExpressionPart::Future(dep_value);
                    }
                    let future = scope.dispatch(expr)?;
                    let body = future.function.body;
                    body(scope, future.bundle)
                }
            };
            results[idx] = Some(value);
        }
        Ok(results.into_iter().map(|r| r.expect("every node should be executed")).collect())
    }

    /// Kahn's algorithm: produce a list of node indices in an order that respects every `deps`
    /// edge. Errors if a cycle is somehow present (shouldn't happen given how `NodeId`s are
    /// minted, but kept as a defensive check).
    fn topo_order(&self) -> Result<Vec<usize>, String> {
        let n = self.nodes.len();
        let mut in_degree = vec![0usize; n];
        let mut successors: Vec<Vec<usize>> = vec![Vec::new(); n];
        for (i, node) in self.nodes.iter().enumerate() {
            for dep in &node.deps {
                in_degree[i] += 1;
                successors[dep.0].push(i);
            }
        }
        let mut queue: VecDeque<usize> =
            (0..n).filter(|&i| in_degree[i] == 0).collect();
        let mut order = Vec::with_capacity(n);
        while let Some(i) = queue.pop_front() {
            order.push(i);
            for &j in &successors[i] {
                in_degree[j] -= 1;
                if in_degree[j] == 0 {
                    queue.push_back(j);
                }
            }
        }
        if order.len() != n {
            return Err("scheduler DAG contains a cycle".to_string());
        }
        Ok(order)
    }
}

impl<'a> Default for Scheduler<'a> {
    fn default() -> Self { Self::new() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::builtins::default_scope;
    use crate::parse::kexpression::{ExpressionPart, KExpression, KLiteral};

    fn let_expr<'a>(name: &str, value: f64) -> KExpression<'a> {
        KExpression {
            parts: vec![
                ExpressionPart::Token("LET".into()),
                ExpressionPart::Token(name.into()),
                ExpressionPart::Token("=".into()),
                ExpressionPart::Literal(KLiteral::Number(value)),
            ],
        }
    }

    #[test]
    fn executes_independent_futures_in_order() {
        let mut scope = default_scope();
        let f1 = scope.dispatch(let_expr("x", 1.0)).unwrap();
        let f2 = scope.dispatch(let_expr("y", 2.0)).unwrap();

        let mut sched = Scheduler::new();
        sched.add(f1);
        sched.add(f2);

        let results = sched.execute(&mut scope).unwrap();

        assert_eq!(results.len(), 2);
        assert!(matches!(results[0], KObject::Number(n) if *n == 1.0));
        assert!(matches!(results[1], KObject::Number(n) if *n == 2.0));
        assert!(scope.data.contains_key("x"));
        assert!(scope.data.contains_key("y"));
    }

    #[test]
    fn dependency_runs_after_its_predecessor() {
        let mut scope = default_scope();
        let f1 = scope.dispatch(let_expr("a", 10.0)).unwrap();
        let f2 = scope.dispatch(let_expr("b", 20.0)).unwrap();

        let mut sched = Scheduler::new();
        let id1 = sched.add(f1);
        let id2 = sched.add_with_deps(f2, vec![id1]);

        let results = sched.execute(&mut scope).unwrap();

        assert!(matches!(results[id1.index()], KObject::Number(n) if *n == 10.0));
        assert!(matches!(results[id2.index()], KObject::Number(n) if *n == 20.0));
    }

    #[test]
    #[should_panic(expected = "does not refer to an existing node")]
    fn rejects_dependency_on_unminted_node() {
        let scope = default_scope();
        let f = scope.dispatch(let_expr("z", 0.0)).unwrap();
        let mut sched = Scheduler::new();
        sched.add_with_deps(f, vec![NodeId(99)]);
    }
}
