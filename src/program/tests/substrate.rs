//! A substrate loads, moves, runs across separate calls, and drops.
//!
//! Kept tiny: the first test is the Miri slate's.

use std::cell::Cell as Tally;

use crate::knot::KValue;
use crate::memory::Bump;
use crate::program::{CellSubstrate, Steps};
use crate::scheduler::{Action, DrainStalled, Placement, Step, StepError, Work};
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
fn add<'graph>(step: Step<'_, 'graph, '_, '_, '_, Steps>) -> Action<'graph, Steps> {
    let (step, state) = step.state();
    let KValue::Number(count) = state else {
        return step.failed(StepError::Stale);
    };
    RAN.with(|ran| ran.set(ran.get() + count));
    step.done()
}

/// A step that refuses to proceed.
fn fail<'graph>(step: Step<'_, 'graph, '_, '_, '_, Steps>) -> Action<'graph, Steps> {
    step.failed(StepError::Stale)
}

/// A root work that adds `count`.
fn adding<'graph>(count: f64) -> Work<'graph, 'graph, Steps> {
    Work {
        step: add,
        state: KValue::Number(count),
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
            let root = running.root();
            let mut scheduler = running.scheduler();
            scheduler
                .run(adding(1.0), root, Placement::Shares)
                .expect("the root work ends");
            assert!(scheduler.graph().is_live(root));
            record
        });
        substrate.with(|running| {
            assert!(matches!(
                running.types().node(record),
                TypeNode::Record { .. }
            ));
            assert_eq!(running.statements().len(), 2 - index);
            let root = running.root();
            let mut scheduler = running.scheduler();
            scheduler
                .run(adding(10.0), root, Placement::Fresh)
                .expect("the root work ends again");
            assert!(scheduler.graph().is_live(root));
        });
    }
    assert_eq!(tally(), 22.0);
    drop(substrates);
}

#[test]
fn a_parse_error_comes_back_from_load() {
    assert!(CellSubstrate::load("foo[2]", "<test>", 2).is_err());
}

/// A stalled drain leaves its cells in the graph, under the root; a later call's drain still runs
/// a fresh root work under that same root.
#[test]
fn a_stalled_substrate_stays_stalled() {
    let mut substrate = loaded("PRINT 1");
    substrate.with(|running| {
        let root = running.root();
        let stalled = running.scheduler().run(
            Work {
                step: fail,
                state: KValue::Null,
            },
            root,
            Placement::Fresh,
        );
        assert_eq!(stalled, Err(DrainStalled::Step(StepError::Stale)));
    });
    RAN.with(|ran| ran.set(0.0));
    substrate.with(|running| {
        let root = running.root();
        let mut scheduler = running.scheduler();
        assert!(!scheduler.graph().is_empty());
        scheduler
            .run(adding(5.0), root, Placement::Fresh)
            .expect("a fresh root work runs beside the stalled one");
    });
    assert_eq!(tally(), 5.0);
}
