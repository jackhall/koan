//! Knot tests: tying cycles out of staged payloads, resolving edges through members, and the plan's
//! range guard.

use super::{Edge, KnotPlan};
use crate::memory::tests::in_cell;

/// A node with one outgoing edge — the shape of a ring or a mutual pair.
#[derive(Clone, Copy)]
struct Link {
    label: u32,
    next: Edge,
}

/// Stage one `Link` per node in the consumer's scratch, from `(label, target)` pairs, before the
/// tie copies them out.
fn staged(plan: &KnotPlan, pairs: &[(u32, u32)]) -> Vec<Link> {
    pairs
        .iter()
        .map(|&(label, target)| Link {
            label,
            next: plan.edge(target).expect("every target is inside the plan"),
        })
        .collect()
}

#[test]
fn two_nodes_that_name_each_other_round_trip() {
    in_cell(|writer| {
        let plan = KnotPlan::new(2);
        let links = staged(&plan, &[(10, 1), (20, 0)]);
        let knot = plan.tie(writer, |edge| links[edge.index() as usize]);

        let first = knot.member(Edge(0));
        let second = first.follow(first.payload().next);
        assert_eq!(second.payload().label, 20);
        assert_eq!(second.index(), Edge(1));
        let back = second.follow(second.payload().next);
        assert_eq!(back.index(), first.index());
        assert_eq!(back.payload().label, 10);
    });
}

#[test]
fn a_node_may_name_itself() {
    in_cell(|writer| {
        let plan = KnotPlan::new(1);
        let links = staged(&plan, &[(7, 0)]);
        let knot = plan.tie(writer, |edge| links[edge.index() as usize]);

        let only = knot.member(Edge(0));
        let again = only.follow(only.payload().next);
        assert_eq!(again.index(), only.index());
        assert_eq!(again.payload().label, 7);
    });
}

#[test]
fn a_ring_walks_back_to_its_start() {
    in_cell(|writer| {
        let plan = KnotPlan::new(3);
        let links = staged(&plan, &[(0, 1), (1, 2), (2, 0)]);
        let knot = plan.tie(writer, |edge| links[edge.index() as usize]);

        let start = knot.member(Edge(0));
        let mut at = start;
        let mut seen = Vec::new();
        for _ in 0..3 {
            seen.push(at.payload().label);
            at = at.follow(at.payload().next);
        }
        assert_eq!(seen, [0, 1, 2]);
        assert_eq!(at.index(), start.index());
        assert_eq!(at.knot().len(), 3);
    });
}

/// A node with any number of outgoing edges, held as a run the tie closure writes into the region.
#[derive(Clone, Copy)]
struct Fan<'cell> {
    label: u32,
    out: &'cell [Edge],
}

#[test]
fn a_payload_holds_a_run_of_edges_written_during_the_tie() {
    in_cell(|writer| {
        let plan = KnotPlan::new(3);
        let adjacency: Vec<Vec<Edge>> = [&[1u32, 2][..], &[], &[0, 1, 2]]
            .iter()
            .map(|targets| targets.iter().map(|&t| plan.edge(t).unwrap()).collect())
            .collect();
        let knot = plan.tie(writer, |edge| {
            let targets = &adjacency[edge.index() as usize];
            Fan {
                label: edge.index() * 100,
                out: writer.fill(targets.len(), |position| targets[position]),
            }
        });

        let hub = knot.member(Edge(2));
        let reached: Vec<u32> = hub
            .payload()
            .out
            .iter()
            .map(|&edge| hub.follow(edge).payload().label)
            .collect();
        assert_eq!(reached, [0, 100, 200]);
        assert!(knot.member(Edge(1)).payload().out.is_empty());
        assert_eq!(knot.member(Edge(0)).payload().out.len(), 2);
    });
}

#[test]
fn a_plan_mints_edges_only_below_its_count() {
    let plan = KnotPlan::new(4);
    assert_eq!(plan.count(), 4);
    assert_eq!(plan.edge(3), Some(Edge(3)));
    assert_eq!(plan.edge(4), None);
    assert_eq!(KnotPlan::new(0).edge(0), None);
}

#[test]
#[should_panic(expected = "edge 4 resolved against a knot of 3 nodes")]
fn an_edge_from_a_larger_plan_panics_at_resolve() {
    let wide = KnotPlan::new(5);
    let stray = wide.edge(4).unwrap();
    in_cell(|writer| {
        let knot = KnotPlan::new(3).tie(writer, |edge| edge.index());
        knot.member(stray);
    });
}

#[test]
fn an_empty_knot_has_no_members() {
    in_cell(|writer| {
        let knot = KnotPlan::new(0).tie(writer, |_| -> u32 { unreachable!("no node to build") });
        assert!(knot.is_empty());
        assert_eq!(knot.len(), 0);
        assert_eq!(knot.members().count(), 0);
    });
}

#[test]
fn members_visit_every_node_in_index_order() {
    in_cell(|writer| {
        let mut built = Vec::new();
        let knot = KnotPlan::new(5).tie(writer, |edge| {
            built.push(edge.index());
            edge.index() * edge.index()
        });
        assert_eq!(built, [0, 1, 2, 3, 4]);
        let read: Vec<(u32, u32)> = knot
            .members()
            .map(|member| (member.index().index(), *member.payload()))
            .collect();
        assert_eq!(read, [(0, 0), (1, 1), (2, 4), (3, 9), (4, 16)]);
    });
}

#[test]
fn a_member_is_equal_only_to_its_own_node() {
    in_cell(|writer| {
        let tie = |pairs: &[(u32, u32)]| {
            let plan = KnotPlan::new(pairs.len() as u32);
            let links = staged(&plan, pairs);
            plan.tie(writer, |edge| links[edge.index() as usize])
        };
        let knot = tie(&[(1, 1), (2, 0)]);
        let other = tie(&[(1, 1), (2, 0)]);
        let (first, second) = (knot.member(Edge(0)), knot.member(Edge(1)));
        assert!(first != second, "two indices of one knot");
        assert!(first != other.member(Edge(0)), "one index of two knots");
        assert!(first == second.follow(second.payload().next));
        assert!(knot == first.knot() && knot != other);
    });
}
