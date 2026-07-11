//! AC 1's door-level pin: a carrier a builtin obtains straight from a `StepAllocator` door cannot
//! be stashed past its construction step. `drive_step_allocator` hands the allocator to a `for<'b>`
//! closure (the step tail's rank-2 shape); storing a door product into an outer `Option` makes the
//! brand lifetime `'b` escape the closure, which the borrow checker rejects. This is the half the
//! run-loop chokepoint guard cannot reach: here the branded carrier is in builtin-shaped hands.

use koan::step_fixture::{drive_step_allocator, KObject};

fn main() {
    let mut escaped = None;
    drive_step_allocator(|allocator| {
        // Smuggle the door product out of its step: `'b` escapes into `escaped`.
        escaped = allocator.alloc_object_scalar(&KObject::Number(1.0));
    });
    let _ = escaped;
}
