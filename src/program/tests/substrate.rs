//! A substrate loads, moves, runs across separate calls, and drops.
//!
//! Kept tiny: the first test is the Miri slate's.

use std::cell::Cell as Tally;

use crate::knot::KValue;
use crate::memory::Bump;
use crate::program::CellSubstrate;
use crate::scheduler::{Action, Context, DrainStalled, Resume, Spawns, State, StepError};
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
    _: &mut Context<'graph, '_, '_, '_>,
    resume: Resume<'graph, '_, '_>,
    _: &mut Spawns<'graph>,
) -> Action<'graph> {
    let State::Value(KValue::Number(count)) = resume.state else {
        return Action::failed(StepError::Stale);
    };
    RAN.with(|ran| ran.set(ran.get() + count));
    Action::done()
}

/// A step that refuses to proceed.
fn fail<'graph>(
    _: &mut Context<'graph, '_, '_, '_>,
    _: Resume<'graph, '_, '_>,
    _: &mut Spawns<'graph>,
) -> Action<'graph> {
    Action::failed(StepError::Stale)
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
            scheduler
                .admit(add, State::Value(KValue::Number(1.0)))
                .expect("the slab admits");
            scheduler.run().expect("the drain runs to empty");
            assert!(scheduler.is_empty());
            record
        });
        substrate.with(|running| {
            assert!(matches!(
                running.types().node(record),
                TypeNode::Record { .. }
            ));
            assert_eq!(running.statements().len(), 2 - index);
            let mut scheduler = running.scheduler();
            scheduler
                .admit(add, State::Value(KValue::Number(10.0)))
                .expect("the slab admits");
            scheduler.run().expect("the drain runs to empty again");
            assert!(scheduler.is_empty());
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
        scheduler
            .admit(fail, State::Empty)
            .expect("the slab admits");
        assert_eq!(scheduler.run(), Err(DrainStalled::Step(StepError::Stale)));
    });
    substrate.with(|running| {
        assert_eq!(running.scheduler().run(), Err(DrainStalled::CellsLive));
    });
}
