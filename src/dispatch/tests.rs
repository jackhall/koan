//! Dispatch's suites, over programs run end to end: each is loaded under [`Koan`], run, and read
//! back as what it wrote to either sink.

mod annotation;
mod ascription;
mod boundary;
mod contributions;
mod generic;
mod instances;
mod programs;
mod quotes;
mod rankings;
mod rules;
mod selection;
mod statics;
mod surface;
mod tail;

use std::cell::RefCell;

use crate::program::{CellSubstrate, Outcome, Output};

use super::Koan;

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
    let mut substrate = match CellSubstrate::load::<Koan>(source, "<test>", 8, output()) {
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
