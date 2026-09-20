//! The barrier node: a function member behind an opaque view, and what crossing it does.
//!
//! [modules](../../../roadmap/rewrite/modules.md) teaches a call to go through the barrier; this
//! item only lays the node down, so these pin what it holds and what `values` sees through it.
//! Copying one is `copy::a_copied_barrier_outlives_its_home`, on the Miri slate.

use crate::memory::{CellGraph, ReleaseAbsorption};
use crate::values::{Knotted as _, Resolved, Value, Weight};

use super::super::{Coerced, Knotted, Node, coerced};
use super::{Step, callable, copy, declared, with_fixture};

const SOURCE: &str = "\
NEWTYPE Dist = Number
LET f = (FN :{} -> Number = (1))";

/// What `coerced` prices a barrier over `underlying` at: the knot's run header, its one node, the
/// barrier beside it and the whole knot the function behind it brings.
fn priced(underlying: Knotted<'_, '_>) -> Weight {
    Weight::flat::<usize>()
        .plus(Weight::flat::<Node<'_, '_>>())
        .plus(Weight::flat::<Coerced<'_, '_>>())
        .plus(underlying.weight())
}

#[test]
fn a_barrier_holds_its_view_and_the_function_behind_it() {
    with_fixture(|fixture| {
        let lines = fixture.parse(SOURCE);
        let mut graph: CellGraph<'_, Step> = CellGraph::new(1, copy);
        let home = graph.create(None).unwrap();
        graph
            .enter(home, |context| {
                let writer = context.writer();
                let activation = fixture.run(writer, &lines, home.into(), &[]);
                let f = callable(fixture, activation, "f");
                let dist = declared(fixture, activation, "Dist");
                let knot = coerced(writer, f, f.ktype(), dist, dist, f.ktype());
                let barrier = Knotted::of(knot, 0);

                assert_eq!(knot.len(), 1, "a barrier is a one-node knot of its own");
                let node = barrier.coerced().expect("a barrier node");
                assert_eq!(node.underlying(), f);
                assert_eq!(node.ktype(), f.ktype());
                assert_eq!(node.declared(), dist);
                assert_eq!(node.from(), dist);
                assert_eq!(node.to(), f.ktype());
                assert_eq!(barrier.weight(), priced(f));
                assert_eq!(node.knot_weight(), barrier.weight());
            })
            .unwrap();
        graph.release(home, ReleaseAbsorption::IntoHolder).unwrap();
    });
}

#[test]
fn values_sees_a_barrier_as_the_function_it_stands_for() {
    with_fixture(|fixture| {
        let lines = fixture.parse(SOURCE);
        let mut graph: CellGraph<'_, Step> = CellGraph::new(1, copy);
        let home = graph.create(None).unwrap();
        graph
            .enter(home, |context| {
                let writer = context.writer();
                let activation = fixture.run(writer, &lines, home.into(), &[]);
                let f = callable(fixture, activation, "f");
                let barrier = Knotted::of(
                    coerced(writer, f, f.ktype(), f.ktype(), f.ktype(), f.ktype()),
                    0,
                );
                let value = Value::Knotted(barrier);

                assert!(matches!(barrier.resolve(), Resolved::Function));
                assert_eq!(value.as_callable(), Some(barrier));
                assert_eq!(value.as_opaque(), Some(barrier));
                assert_eq!(value.as_module(), None);
                assert!(
                    value.as_circular().is_none(),
                    "a barrier is opaque to `values`"
                );
                assert_eq!(barrier.ktype(), f.ktype());

                // A view of a view stacks barriers rather than collapsing them: each one holds the
                // substitution it was built at.
                let outer = Knotted::of(
                    coerced(writer, barrier, f.ktype(), f.ktype(), f.ktype(), f.ktype()),
                    0,
                );
                assert_eq!(outer.coerced().expect("a barrier").underlying(), barrier);
                assert!(outer.weight().bytes() > barrier.weight().bytes());
            })
            .unwrap();
        graph.release(home, ReleaseAbsorption::IntoHolder).unwrap();
    });
}
