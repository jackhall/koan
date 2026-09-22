//! Shared scaffolding for `program`'s suites: a program loaded over the miniature evaluator, and a
//! later root work that reads top-level bindings after the drain.

mod boundary;
mod evaluator;
mod programs;
mod substrate;

use std::cell::RefCell;

use crate::knot::{KValue, Knotted};
use crate::program::{CellSubstrate, KBirth, KBundle, KState};
use crate::scheduler::{Action, Step, StepError};
use crate::scope::{Coordinate, Target};
use crate::values::Value;

use evaluator::{Mini, record};

thread_local! {
    /// The top-level names the next [`inspecting`] step reads, in order.
    static WANTED: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// `source` loaded over the miniature evaluator on a slab of `cap` cells.
fn loaded(source: &str, cap: u32) -> CellSubstrate {
    CellSubstrate::load::<Mini>(source, "<test>", cap)
        .unwrap_or_else(|error| panic!("`{source}` loads: {error:?}"))
}

/// Run `substrate`'s program, then read `names` back through a second root work in a separate call,
/// and hand back what each read.
fn run_and_read(substrate: &mut CellSubstrate, names: &[&str]) -> Vec<String> {
    substrate.with(|running| running.run().expect("the program runs to completion"));
    read_back(substrate, names)
}

/// Read `names` through a root work resumed from what the top level left at rest.
fn read_back(substrate: &mut CellSubstrate, names: &[&str]) -> Vec<String> {
    evaluator::reset();
    WANTED
        .with(|wanted| *wanted.borrow_mut() = names.iter().map(|name| name.to_string()).collect());
    substrate.with(|running| {
        running
            .inspect(inspecting)
            .expect("the inspection runs to completion")
    });
    evaluator::recorded()
}

/// A root work born from the top level's resting view: it records each wanted binding and leaves
/// the view at rest again, for the next inspection.
fn inspecting<'graph>(step: Step<'_, 'graph, '_, '_, '_, KBundle>) -> Action<'graph, KBundle> {
    let (step, state) = step.state();
    let KState::Born(birth @ KBirth::Inspect { program, view }) = state else {
        return step.failed(StepError::Refused);
    };
    for name in WANTED.with(|wanted| wanted.borrow().clone()) {
        let slot = program.binding(&name).expect("a top-level binding");
        let value = view.read(Coordinate::Activation {
            hops: 0,
            target: Target::Local(slot),
        });
        record(describe(value));
    }
    step.leave(birth)
}

/// A value in a form a test can assert on.
fn describe(value: KValue<'_, '_>) -> String {
    match value {
        Value::Number(number) => number.to_string(),
        Value::Bool(flag) => flag.to_string(),
        Value::Null => String::from("null"),
        Value::Str(text) => format!("{text:?}"),
        Value::List(list) => {
            let cells: Vec<_> = list.cells().iter().map(|cell| describe(*cell)).collect();
            format!("[{}]@{:p}", cells.join(" "), list)
        }
        Value::Knotted(member) => describe_member(member),
        _ => String::from("other"),
    }
}

/// A knot member: a function by the knot it sits in, a module by its members, a data node by its
/// knot.
fn describe_member(member: Knotted<'_, '_>) -> String {
    let knot = member
        .member()
        .knot()
        .members()
        .next()
        .expect("a knot has a member")
        .payload();
    if let Some(module) = member.module() {
        let members: Vec<_> = module
            .members()
            .iter()
            .map(|value| describe(*value))
            .collect();
        return format!("module({})", members.join(", "));
    }
    if member.function().is_some() {
        return format!("fn in {knot:p}");
    }
    format!("node in {knot:p}")
}
