//! Tarjan against brute force: over a random digraph the components partition the nodes, each is a
//! mutual-reachability class, and the emission order is reverse topological on the condensation.

use proptest::prelude::*;

use super::strongly_connected_components;
use crate::bump::Bump;

/// A digraph of up to twelve nodes as adjacency lists.
fn digraph() -> impl Strategy<Value = Vec<Vec<usize>>> {
    (1usize..12).prop_flat_map(|count| {
        proptest::collection::vec(proptest::collection::vec(0..count, 0..4), count)
    })
}

/// `reach[a][b]` — whether `b` is reachable from `a` by zero or more edges.
fn reachability(edges: &[Vec<usize>]) -> Vec<Vec<bool>> {
    let count = edges.len();
    let mut reach = vec![vec![false; count]; count];
    for (node, row) in reach.iter_mut().enumerate() {
        row[node] = true;
        let mut stack = vec![node];
        while let Some(at) = stack.pop() {
            for &next in &edges[at] {
                if !row[next] {
                    row[next] = true;
                    stack.push(next);
                }
            }
        }
    }
    reach
}

proptest! {
    #![proptest_config(ProptestConfig { cases: crate::tests::case_share(1, 1), ..ProptestConfig::default() })]

    #[test]
    fn components_are_the_mutual_reachability_classes_in_reverse_topological_order(
        edges in digraph()
    ) {
        let scratch = Bump::new();
        let borrowed: Vec<&[usize]> = edges.iter().map(Vec::as_slice).collect();
        let components = strongly_connected_components(&scratch, &borrowed);
        let reach = reachability(&edges);
        let count = edges.len();

        let mut component_of = vec![usize::MAX; count];
        for (index, component) in components.iter().enumerate() {
            for &node in component.iter() {
                prop_assert_eq!(component_of[node], usize::MAX, "node {} emitted twice", node);
                component_of[node] = index;
            }
        }
        prop_assert!(component_of.iter().all(|&index| index != usize::MAX));

        for a in 0..count {
            for b in 0..count {
                prop_assert_eq!(
                    component_of[a] == component_of[b],
                    reach[a][b] && reach[b][a],
                    "nodes {} and {}", a, b
                );
                // An edge leaving a component names one emitted no later than it.
                if edges[a].contains(&b) {
                    prop_assert!(component_of[b] <= component_of[a]);
                }
            }
        }
    }
}

#[test]
fn a_chain_far_longer_than_a_thread_stack_allows_recursing_over_is_walked() {
    // Each node references the next, so a walk from the first reaches every other before it closes
    // one: a recursive walk would be as deep as the chain.
    let count = if cfg!(miri) { 1_000 } else { 100_000 };
    let edges: Vec<Vec<usize>> = (0..count)
        .map(|node| {
            if node + 1 < count {
                vec![node + 1]
            } else {
                vec![]
            }
        })
        .collect();
    let borrowed: Vec<&[usize]> = edges.iter().map(Vec::as_slice).collect();
    let scratch = Bump::new();
    let components = strongly_connected_components(&scratch, &borrowed);
    let emitted: Vec<usize> = components.iter().map(|component| component[0]).collect();
    assert!(components.iter().all(|component| component.len() == 1));
    assert_eq!(emitted, (0..count).rev().collect::<Vec<_>>());
}
