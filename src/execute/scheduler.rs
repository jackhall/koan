use std::collections::VecDeque;

use crate::dispatch::kobject::KObject;
use crate::dispatch::scope::{KFuture, Scope};

/// Stable handle to a node in a `Scheduler`'s DAG. Returned by `Scheduler::add` and
/// `add_with_deps`, and used to declare a later future's dependencies on earlier ones.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct NodeId(usize);

impl NodeId {
    pub fn index(self) -> usize { self.0 }
}

/// One vertex of the scheduler DAG: the deferred call (`KFuture`) plus the ids of nodes whose
/// execution must complete before this one runs.
struct Node<'a> {
    future: KFuture<'a>,
    deps: Vec<NodeId>,
}

/// Holds a directed acyclic graph of `KFuture`s and runs them in dependency order. Callers
/// register futures via `add`/`add_with_deps`, which return a `NodeId` that can be reused as a
/// dependency for later additions; `execute` performs a Kahn-style topological sort, invokes
/// each function body against the supplied root `Scope`, and yields the produced `KObject`
/// references in submission order.
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
    /// node already in the scheduler — `NodeId`s are only minted by `add`/`add_with_deps`, so
    /// edges always point backwards in submission order and the graph is acyclic by construction.
    pub fn add_with_deps(&mut self, future: KFuture<'a>, deps: Vec<NodeId>) -> NodeId {
        let id = NodeId(self.nodes.len());
        for dep in &deps {
            assert!(
                dep.0 < id.0,
                "scheduler dependency NodeId({}) does not refer to an existing node",
                dep.0,
            );
        }
        self.nodes.push(Node { future, deps });
        id
    }

    pub fn len(&self) -> usize { self.nodes.len() }
    pub fn is_empty(&self) -> bool { self.nodes.is_empty() }

    /// Topologically sort the DAG and run each future against `scope`. Returns the produced
    /// `KObject`s indexed by `NodeId` (i.e. submission order), or an error string if the graph
    /// somehow contains a cycle.
    pub fn execute(self, scope: &mut Scope<'a>) -> Result<Vec<&'a KObject<'a>>, String> {
        let order = self.topo_order()?;
        let n = self.nodes.len();
        let mut nodes: Vec<Option<Node<'a>>> = self.nodes.into_iter().map(Some).collect();
        let mut results: Vec<Option<&'a KObject<'a>>> = vec![None; n];
        for idx in order {
            let node = nodes[idx].take().expect("topological order must not revisit a node");
            let body = node.future.function.body;
            results[idx] = Some(body(scope, node.future.bundle));
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

    fn let_expr(name: &str, value: f64) -> KExpression {
        KExpression {
            parts: vec![
                ExpressionPart::Token("let".into()),
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
