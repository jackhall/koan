//! What the spawn and delivery tests share. A native step is a bare `fn` and carries no closure
//! state, so what a step observes it records here, for the test around it to read back — the
//! cells it ran in included, since a unit has no cell until the drain reaches it.

use std::cell::RefCell;

use crate::knot::KValue;
use crate::memory::CellHandle;
use crate::scheduler::{Birth, NativeStep, State, Unit, Work};

thread_local! {
    static SEEN: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    static CELLS: RefCell<Vec<CellHandle>> = const { RefCell::new(Vec::new()) };
}

/// Forget everything the previous test recorded.
pub fn reset() {
    SEEN.with(|seen| seen.borrow_mut().clear());
    CELLS.with(|cells| cells.borrow_mut().clear());
}

/// A unit born in the slab with `state`, for a test to submit with no dependencies.
pub fn slab<'graph>(step: NativeStep<'graph>, state: State<'graph, 'graph>) -> Unit<'graph> {
    Unit {
        birth: Birth::Slab,
        work: Work { step, state },
    }
}

/// Record the cell a step ran in, for a test that watches its release land.
pub fn record_cell(cell: CellHandle) {
    CELLS.with(|cells| cells.borrow_mut().push(cell));
}

/// Every cell recorded since the last reset, in the order the drain ran them.
pub fn recorded_cells() -> Vec<CellHandle> {
    CELLS.with(|cells| cells.borrow().clone())
}

/// Record one observation, in the order the drain produced it.
pub fn record(what: String) {
    SEEN.with(|seen| seen.borrow_mut().push(what));
}

/// Everything recorded since the last reset.
pub fn recorded() -> Vec<String> {
    SEEN.with(|seen| seen.borrow().clone())
}

/// A value in a form the test can assert on, where naming its brand would be a lifetime it has no
/// way to write.
pub fn describe(value: KValue<'_, '_>) -> String {
    match value {
        KValue::Number(number) => number.to_string(),
        KValue::Str(text) => text.to_string(),
        _ => String::from("neither a number nor text"),
    }
}
