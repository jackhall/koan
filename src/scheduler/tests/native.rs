//! What the spawn and delivery tests share. A native step is a bare `fn` and carries no closure
//! state, so what a step observes it records here, for the test around it to read back.

use std::cell::RefCell;

use crate::function::KValue;

thread_local! {
    static SEEN: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// Forget everything the previous test recorded.
pub fn reset() {
    SEEN.with(|seen| seen.borrow_mut().clear());
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
