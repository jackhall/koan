//! The continuation round trip: a cell born at `'graph`, entered, storing a successor at its own
//! region brand, and entered again to take it back.
//!
//! It drives the raw context's slot doors on purpose — the doors a step reaches only through
//! [`Step::park`](crate::scheduler::Step::park) — so it builds its own graph over the scheduler's
//! families rather than a [`Graph`](crate::scheduler::Graph).

use crate::knot::KValue;
use crate::memory::CellGraph;
use crate::scheduler::continuation::{
    CellPlace, Continuation, ContinuationFamily, Provenance, ScratchFamily,
};
use crate::scheduler::delivery::KDelivery;
use crate::scheduler::{Action, ScratchState, State, Step};

type Graph<'graph> = CellGraph<'graph, ContinuationFamily, ScratchFamily, KDelivery>;

fn place() -> Provenance {
    Provenance {
        place: CellPlace::Slab,
        destination: None,
        unit: None,
    }
}

/// A step that does nothing, named only so a continuation has a pointer to hold.
fn inert<'graph>(
    step: Step<'_, 'graph, '_, '_, '_>,
    _: State<'graph, '_>,
    _: Option<ScratchState<'graph, '_, '_>>,
) -> Action<'graph> {
    step.done()
}

#[test]
fn a_birth_continuation_comes_back_at_the_step_brand_and_a_successor_survives_to_the_next_step() {
    let mut graph: Graph<'static> = CellGraph::new(2, crate::values::verdict);
    let cell = graph
        .create(Some(Continuation::Native {
            step: inert,
            provenance: place(),
            state: State::Value(KValue::Number(1.0)),
        }))
        .expect("a fresh slab admits a cell");

    // The birth continuation comes back at `'here`; the successor stored is built at `'here` too,
    // out of a borrow of this cell's own region.
    graph
        .enter(cell, |context| {
            let Continuation::Native { state, .. } =
                context.continuation().expect("the birth continuation");
            assert!(matches!(state, State::Value(KValue::Number(one)) if one == 1.0));
            // A value borrowing this cell's own region, so the successor genuinely carries a
            // region borrow across the slot and back.
            let here = crate::values::text(context.writer(), "here");
            context.store_successor(Continuation::Native {
                step: inert,
                provenance: place(),
                state: State::Value(here),
            });
            let there = crate::values::text(context.scratch_writer(), "there");
            context.store_scratch_state(ScratchState::Value(there));
        })
        .expect("the cell enters");

    graph
        .enter(cell, |context| {
            let Continuation::Native { state, .. } =
                context.continuation().expect("the stored successor");
            assert!(matches!(state, State::Value(KValue::Str("here"))));
            let scratch = context.scratch_state().expect("the stored scratch state");
            assert!(matches!(scratch, ScratchState::Value(KValue::Str("there"))));
        })
        .expect("the cell enters a second time");

    graph
        .release(cell, crate::memory::ReleaseAbsorption::IntoHolder)
        .expect("the cell releases");
    assert!(graph.is_empty());
}
