//! The load door: a graph stands up over its build root and its run root, the load runs once in
//! the build root, and both roots release to an empty graph.

use crate::memory::Bump;
use crate::scheduler::Graph;
use crate::scheduler::tests::bundle::Native;
use crate::type_lattice::{Habitat, Root};
use crate::values::Half;

#[test]
fn the_load_stands_two_roots_and_lays_the_build_roots_header() {
    let type_region = Bump::new();
    let (mut graph, roots, out) = Graph::<'_, Native>::load(2, |step| {
        let _ = Half::of(step.writer(), step.header()).own_or_lay();
        Ok::<_, ()>((Root::seeded(&type_region), 7))
    })
    .expect("the load succeeds");
    assert_eq!(out, 7, "the load's own result comes back");
    assert_ne!(roots.build, roots.run);
    assert!(graph.is_live(roots.build) && graph.is_live(roots.run));
    assert_eq!(
        graph.table_len(roots.build),
        Ok(Some(0)),
        "the build root's header is laid, with no specialization in its table"
    );
    assert_eq!(
        graph.table_len(roots.run),
        Ok(None),
        "the run root lays nothing"
    );
    graph
        .release_root(roots.run)
        .expect("the run root releases");
    graph
        .release_root(roots.build)
        .expect("the build root releases");
    assert!(graph.is_empty());
}

#[test]
fn a_refused_load_hands_its_refusal_back() {
    let refused = Graph::<'_, Native>::load(2, |_| Err::<(&Root<'_>, ()), _>("refused"));
    assert_eq!(refused.err(), Some("refused"));
}
