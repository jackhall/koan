//! The unwrap half of the step-brand discipline: the sole exit `StepCarried::seal_at_step` is
//! `pub(super)` (visible only within `machine::execute`), so an external crate cannot call it to
//! strip the brand and recover the lifetime-free carrier. The type is nameable (via `step_fixture`),
//! but its exit is not reachable.

use koan::step_fixture::drive_step;

fn main() {
    drive_step(|carrier| {
        // `seal_at_step` is `pub(super)`; an external crate cannot call it.
        let _escape = carrier.seal_at_step(unimplemented!());
    });
}
