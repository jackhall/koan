//! What the drain tests share. A native step is a bare `fn` and carries no closure state, so what
//! a step observes it records here, for the test around it to read back.

use std::cell::RefCell;

use crate::knot::KValue;
use crate::scheduler::tests::bundle::{Native, TestStep};
use crate::scheduler::{Placement, Request, Use, Work};

thread_local! {
    static SEEN: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// Forget everything the previous test recorded.
pub fn reset() {
    SEEN.with(|seen| seen.borrow_mut().clear());
}

/// A root work: `step`, born holding `state` at `'graph`.
pub fn work<'graph>(
    step: TestStep<'graph>,
    state: KValue<'graph, 'graph>,
) -> Work<'graph, 'graph, Native> {
    Work { step, state }
}

/// A child in a region of its own, asked with `use_`, born holding `state`.
pub fn fresh<'graph, 'here>(
    step: TestStep<'graph>,
    use_: Use,
    state: KValue<'graph, 'here>,
) -> Request<'graph, 'here, Native> {
    Request {
        placement: Placement::Fresh,
        use_,
        work: Work { step, state },
    }
}

/// A child sharing its spawner's region, asked with `use_`, born holding `state`.
pub fn shares<'graph, 'here>(
    step: TestStep<'graph>,
    use_: Use,
    state: KValue<'graph, 'here>,
) -> Request<'graph, 'here, Native> {
    Request {
        placement: Placement::Shares,
        use_,
        work: Work { step, state },
    }
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

/// A text value as "<text>@<address>": what it says, and the bytes it says it from, so a copy is
/// visible as a different address.
pub fn where_text(value: KValue<'_, '_>) -> String {
    match value {
        KValue::Str(text) => format!("{text}@{:?}", text.as_ptr()),
        _ => String::from("not text"),
    }
}
