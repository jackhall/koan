//! Shared scaffolding for `program`'s suites: a program loaded over the miniature evaluator, and a
//! later root work that reads top-level bindings — or compares two — after the drain.

mod boundary;
mod evaluator;
mod programs;
mod substrate;

use std::cell::RefCell;

use crate::knot::{KValue, Knotted};
use crate::memory::Bump;
use crate::program::record::Program;
use crate::program::{CellSubstrate, KBirth, KBundle, KState};
use crate::scheduler::{Action, Step, StepError};
use crate::scope::{Coordinate, Target};
use crate::type_lattice::display_name;
use crate::values::{Knotted as _, Link, Value};

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

/// Compare the top-level bindings `left` and `right` through a root work resumed from what the top
/// level left at rest, and hand back what `equals` answered.
fn compare_back(substrate: &mut CellSubstrate, left: &str, right: &str) -> String {
    evaluator::reset();
    WANTED.with(|wanted| *wanted.borrow_mut() = vec![left.to_string(), right.to_string()]);
    substrate.with(|running| {
        running
            .inspect(comparing)
            .expect("the comparison runs to completion")
    });
    evaluator::recorded().concat()
}

/// A root work born from the top level's resting view: it records each wanted binding and leaves
/// the view at rest again, for the next inspection.
fn inspecting<'graph>(step: Step<'_, 'graph, '_, '_, '_, KBundle>) -> Action<'graph, KBundle> {
    let (step, state) = step.state();
    let KState::Born(birth @ KBirth::Inspect { program, view }) = state else {
        return step.failed(StepError::Refused);
    };
    for name in WANTED.with(|wanted| wanted.borrow().clone()) {
        record(describe(wanted(program, &view, &name), program));
    }
    step.leave(birth)
}

/// [`inspecting`]'s sibling: it records whether the two wanted bindings are equal.
fn comparing<'graph>(step: Step<'_, 'graph, '_, '_, '_, KBundle>) -> Action<'graph, KBundle> {
    let (step, state) = step.state();
    let KState::Born(birth @ KBirth::Inspect { program, view }) = state else {
        return step.failed(StepError::Refused);
    };
    let names = WANTED.with(|wanted| wanted.borrow().clone());
    let [left, right] = [&names[0], &names[1]].map(|name| wanted(program, &view, name));
    let bump = Bump::new();
    record(format!("{:?}", left.equals(&right, program.types(), &bump)));
    step.leave(birth)
}

/// The top-level binding `name`, read through `view`.
fn wanted<'graph, 'cell>(
    program: &'graph Program<'graph>,
    view: &crate::knot::KActivationView<'graph, 'cell>,
    name: &str,
) -> KValue<'graph, 'cell> {
    let slot = program.binding(name).expect("a top-level binding");
    view.read(Coordinate::Activation {
        hops: 0,
        target: Target::Local(slot),
    })
}

/// A value in a form a test can assert on.
fn describe<'graph>(value: KValue<'graph, '_>, program: &'graph Program<'graph>) -> String {
    match value {
        Value::Number(number) => number.to_string(),
        Value::Bool(flag) => flag.to_string(),
        Value::Null => String::from("null"),
        Value::Str(text) => format!("{text:?}"),
        Value::List(list) => {
            let cells: Vec<_> = list
                .cells()
                .iter()
                .map(|cell| describe(*cell, program))
                .collect();
            format!("[{}]@{:p}", cells.join(" "), list)
        }
        Value::Type(value) => {
            display_name(value.handle(), program.types(), program.symbols()).to_string()
        }
        Value::Knotted(member) => describe_member(member, program),
        _ => String::from("other"),
    }
}

/// A knot member: a function by the knot it sits in, a module by its members, a quote by its code,
/// its knot's size and its bindings, a data node by its knot.
fn describe_member<'graph>(
    member: Knotted<'graph, '_>,
    program: &'graph Program<'graph>,
) -> String {
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
            .map(|value| describe(*value, program))
            .collect();
        return format!("module({})", members.join(", "));
    }
    if member.function().is_some() {
        return format!("fn in {knot:p}");
    }
    if let Some(code) = member.code() {
        let bound: Vec<_> = code
            .bound()
            .iter()
            .map(|(name, link)| {
                let value = match link {
                    Link::Edge(edge) if member.sibling(*edge) == member => String::from("self"),
                    Link::Edge(_) => String::from("edge"),
                    Link::Value(value) => describe(*value, program),
                };
                format!("{}={value}", program.symbols().display(name.symbol()))
            })
            .collect();
        return format!(
            "#({}) in {} binding [{}]",
            code.body().summary(program.symbols()),
            member.member().knot().len(),
            bound.join(" ")
        );
    }
    format!("node in {knot:p}")
}
