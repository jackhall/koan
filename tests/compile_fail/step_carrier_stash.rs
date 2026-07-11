//! AC 1's pin: a Done-arm carrier cannot be stashed past its construction step. `drive_step` hands
//! the carrier to a `for<'b>` closure (the step tail's rank-2 shape); storing it into an outer
//! `Option` makes the brand lifetime `'b` escape the closure, which the borrow checker rejects.

use koan::step_fixture::drive_step;

fn main() {
    let mut escaped = None;
    drive_step(|carrier| {
        // Smuggle the step-branded carrier out of its step: `'b` escapes into `escaped`.
        escaped = Some(carrier);
    });
    let _ = escaped;
}
