//! The load door: the load runs before the graph stands, and the graph holds one slab root, its run
//! root, which releases to an empty graph.

use crate::memory::Bump;
use crate::scheduler::Graph;
use crate::scheduler::tests::bundle::Native;
use crate::values::BuildBudget;

#[test]
fn the_graph_holds_one_slab_root() {
    let type_region = Bump::new();
    let (mut graph, run, out) = Graph::<'_, Native>::load(1, BuildBudget::default(), || {
        Ok::<_, ()>((crate::values::tests::seeded_tiers(&type_region), 7))
    })
    .expect("the load succeeds");
    assert_eq!(out, 7, "the load's own result comes back");
    assert!(graph.is_live(run));
    assert!(
        graph.root().is_err(),
        "a slab of one holds the run root alone"
    );
    assert_eq!(graph.live_settlements(), 0);
    graph.release_root(run).expect("the run root releases");
    assert!(graph.is_empty());
}

#[test]
fn a_refused_load_hands_its_refusal_back() {
    let refused = Graph::<'_, Native>::load(1, BuildBudget::default(), || {
        Err::<(crate::values::TypeTiers<'_>, ()), _>("refused")
    });
    assert_eq!(refused.err(), Some("refused"));
}
