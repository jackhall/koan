//! Dispatch's suites, over programs run end to end: each is loaded under [`Koan`], run, and read
//! back as what it wrote to either sink.

mod activation;
mod annotation;
mod ascription;
mod boundary;
mod bounds;
mod caching;
mod classes;
mod compositions;
mod contributions;
mod digests;
mod door;
mod frontiers;
mod functions;
mod generate;
mod generic;
mod instances;
mod kept;
mod keys;
mod lexical;
mod literal_splices;
mod loops;
mod memo;
mod modules;
mod named_code;
mod narrowing;
mod paths;
mod programs;
mod quotes;
mod rankings;
mod reads;
mod repeats;
mod replay;
mod resumption;
mod retypes;
mod rules;
mod selection;
mod signatures;
mod spellings;
mod splicing;
mod spreads;
mod statics;
mod surface;
mod tail;
mod templates;
mod trees;
mod types;
mod views;

use std::cell::RefCell;

use crate::program::{CellSubstrate, Outcome, Output};

use super::Koan;
use super::walk::{LoadCounts, counted};
use crate::values::BuildBudget;

thread_local! {
    /// Every line written to either sink since the last [`run`], in order.
    static WRITTEN: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// Sinks that keep what they are handed.
fn output() -> Output {
    fn keep(text: &str) {
        WRITTEN.with(|written| written.borrow_mut().push(text.to_string()));
    }
    Output {
        print: keep,
        error: keep,
    }
}

/// Load `source` under dispatch and run it: every line it wrote, one per write, or the load's
/// refusal as `load: <diagnostic>`.
fn run(source: &str) -> String {
    WRITTEN.with(|written| written.borrow_mut().clear());
    let mut substrate =
        match CellSubstrate::load::<Koan>(source, "<test>", 8, BuildBudget::default(), output()) {
            Ok(substrate) => substrate,
            Err(error) => return format!("load: {error}"),
        };
    let outcome = substrate.with(|running| running.run());
    let written = WRITTEN.with(|written| std::mem::take(&mut *written.borrow_mut()));
    match outcome {
        Ok(Outcome::Completed | Outcome::Uncaught) => written.join("\n"),
        Err(stalled) => format!("stalled: {stalled:?} after {written:?}"),
    }
}

/// What the load counted loading `source`.
fn load_counts(source: &str) -> LoadCounts {
    let (refused, counts) = counted(|| {
        let loaded =
            CellSubstrate::load::<Koan>(source, "<test>", 8, BuildBudget::default(), output());
        loaded.err().map(|error| error.to_string())
    });
    if let Some(error) = refused {
        panic!("the program loads: {error}");
    }
    counts
}

/// Assert that `count`, read off a program generated at size `n`, grows at most linearly: over
/// `n` = 8, 16 and 24 its second step grows it by no more than an eighth past its first, where a
/// quadratic count's grows by up to two thirds again.
fn assert_linear(what: &str, count: impl Fn(usize) -> usize) {
    let [at8, at16, at24] = [8, 16, 24].map(&count);
    let (first, second) = (at16.saturating_sub(at8), at24.saturating_sub(at16));
    assert!(
        second <= first + first / 8,
        "{what} grow faster than linearly: {at8} at 8, {at16} at 16, {at24} at 24"
    );
}
