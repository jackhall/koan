//! The continuation round trip: a cell born at `'graph`, entered, storing a successor at its own
//! region brand, and entered again to take it back.
//!
//! It drives the raw context's slot doors on purpose — the doors a step reaches only through
//! [`Step::park`](crate::scheduler::Step::park) — over the graph a [`Graph`] wraps.

use crate::knot::KValue;
use crate::memory::CellHandle;
use crate::scheduler::continuation::{Continuation, Provenance, Rested};
use crate::scheduler::tests::bundle::{Native, ScratchState, TestGraph};
use crate::scheduler::{Action, Graph, Step};

fn place(cell: CellHandle) -> Provenance {
    Provenance {
        parent: cell,
        report: None,
        home: cell,
    }
}

/// A step that does nothing, named only so a continuation has a pointer to hold.
fn inert<'graph>(step: Step<'_, 'graph, '_, '_, '_, Native>) -> Action<'graph, Native> {
    step.done()
}

#[test]
fn a_birth_continuation_comes_back_at_the_step_brand_and_a_successor_survives_to_the_next_step() {
    let mut graph: TestGraph<'static> = Graph::new(2);
    let root: CellHandle = graph.root().expect("a fresh slab admits a root").into();
    let cells = graph.cells();
    let cell = cells
        .create_tree(
            root,
            Some(Continuation {
                step: inert,
                provenance: place(root),
                state: Rested::Awake(KValue::Number(1.0)),
            }),
        )
        .expect("a root admits a tree child");

    // The birth continuation comes back at `'here`; the successor stored is built at `'here` too,
    // out of a borrow of this cell's own region.
    cells
        .enter(cell, |context| {
            let Continuation { state, .. } =
                context.continuation().expect("the birth continuation");
            assert!(matches!(state, Rested::Awake(KValue::Number(one)) if one == 1.0));
            // A value borrowing this cell's own region, so the successor genuinely carries a
            // region borrow across the slot and back.
            let here = crate::values::text(context.writer(), "here");
            context.store_successor(Continuation {
                step: inert,
                provenance: place(root),
                state: Rested::Awake(here),
            });
            let there = crate::values::text(context.scratch_writer(), "there");
            context.store_scratch_state(ScratchState::Value(there));
        })
        .expect("the cell enters");

    cells
        .enter(cell, |context| {
            let Continuation { state, .. } = context.continuation().expect("the stored successor");
            assert!(matches!(state, Rested::Awake(KValue::Str("here"))));
            let scratch = context.scratch_state().expect("the stored scratch state");
            assert!(matches!(scratch, ScratchState::Value(KValue::Str("there"))));
        })
        .expect("the cell enters a second time");

    cells.release_tree(cell).expect("the cell releases");
    let CellHandle::Slab(root) = root else {
        unreachable!("a root is a slab cell")
    };
    graph.release_root(root).expect("the root releases");
    assert!(graph.is_empty());
}
