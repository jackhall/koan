//! A substrate loads, moves, runs across separate calls, and drops.
//!
//! Kept tiny: the first test is the Miri slate's.

use std::cell::Cell as Tally;

use crate::knot::KValue;
use crate::memory::Bump;
use crate::program::CellSubstrate;
use crate::scheduler::{
    Action, Birth, DrainStalled, NativeStep, ScratchState, State, Step, StepError, Unit, Work,
};
use crate::symbols::BinderSymbol;
use crate::type_lattice::{KType, TypeNode};

thread_local! {
    /// What a step records for the test around it. A step is a bare `fn`, so it carries no
    /// closure state; the tally stands in for one, in the test alone.
    static RAN: Tally<f64> = const { Tally::new(0.0) };
}

fn tally() -> f64 {
    RAN.with(|ran| ran.get())
}

/// A step that adds the number it was born with to the tally, and finishes.
fn add<'graph>(
    step: Step<'_, 'graph, '_, '_, '_>,
    state: State<'graph, '_>,
    _: Option<ScratchState<'graph, '_, '_>>,
) -> Action<'graph> {
    let State::Value(KValue::Number(count)) = state else {
        return step.failed(StepError::Stale);
    };
    RAN.with(|ran| ran.set(ran.get() + count));
    step.done()
}

/// A step that refuses to proceed.
fn fail<'graph>(
    step: Step<'_, 'graph, '_, '_, '_>,
    _: State<'graph, '_>,
    _: Option<ScratchState<'graph, '_, '_>>,
) -> Action<'graph> {
    step.failed(StepError::Stale)
}

/// A unit born in the slab with `state`, for a test to submit with no dependencies.
fn slab<'graph>(step: NativeStep<'graph>, state: State<'graph, 'graph>) -> Unit<'graph> {
    Unit {
        birth: Birth::Slab,
        work: Work { step, state },
    }
}

fn loaded(source: &str) -> CellSubstrate {
    CellSubstrate::load(source, "<test>", 2).expect("the source parses")
}

#[test]
fn two_substrates_load_move_and_run_in_separate_calls() {
    RAN.with(|ran| ran.set(0.0));
    let mut substrates = vec![loaded("PRINT 1"), loaded("PRINT 1\nPRINT 2")];
    substrates.reverse();
    let mut substrates = Box::new(substrates);
    for (index, substrate) in substrates.iter_mut().enumerate() {
        let record = substrate.with(|running| {
            let scratch = Bump::new();
            let x = BinderSymbol::declared("x", running.symbols()).expect("a bindable token");
            let record = running.types().record(&scratch, &[(x, KType::NUMBER)]);
            let mut scheduler = running.scheduler();
            scheduler.submit(slab(add, State::Value(KValue::Number(1.0))), 0);
            scheduler.run().expect("the drain runs to empty");
            assert!(scheduler.graph().is_empty());
            record
        });
        substrate.with(|running| {
            assert!(matches!(
                running.types().node(record),
                TypeNode::Record { .. }
            ));
            assert_eq!(running.statements().len(), 2 - index);
            let mut scheduler = running.scheduler();
            scheduler.submit(slab(add, State::Value(KValue::Number(10.0))), 0);
            scheduler.run().expect("the drain runs to empty again");
            assert!(scheduler.graph().is_empty());
        });
    }
    assert_eq!(tally(), 22.0);
    drop(substrates);
}

#[test]
fn a_parse_error_comes_back_from_load() {
    assert!(CellSubstrate::load("foo[2]", "<test>", 2).is_err());
}

#[test]
fn a_stalled_substrate_stays_stalled() {
    let mut substrate = loaded("PRINT 1");
    substrate.with(|running| {
        let mut scheduler = running.scheduler();
        scheduler.submit(slab(fail, State::Empty), 0);
        assert_eq!(scheduler.run(), Err(DrainStalled::Step(StepError::Stale)));
    });
    substrate.with(|running| {
        assert_eq!(running.scheduler().run(), Err(DrainStalled::CellsLive));
    });
}
