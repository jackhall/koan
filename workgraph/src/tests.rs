//! Crate-wide test scaffolding: a delegating allocation counter over the system allocator,
//! installed for the library's own unit-test binary only.
//!
//! The scheduler's steady-state claim — a slot that parks and wakes on the same shape stops
//! allocating once its rows have grown — is a claim about allocation *counts*, and a count is the
//! only way to state it that does not drift with the machine. [`allocation_count`] reads a
//! thread-local tally, so a test brackets the run it is measuring and asserts the delta; the
//! harness runs tests concurrently, and a process-wide counter would tally every other test's
//! traffic into that bracket.
//!
//! Wrapping [`System`](std::alloc::System) rather than replacing it keeps the counted build on the
//! same allocator the shipped one uses. Koan carries its own copy of this scaffolding for its own
//! baselines; workgraph depends on nothing above it, so it counts with its own.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static THREAD_ALLOCATIONS: Cell<u64> = const { Cell::new(0) };
}

/// The number of heap allocations this thread has made since it started.
pub(crate) fn allocation_count() -> u64 {
    THREAD_ALLOCATIONS.with(Cell::get)
}

/// Delegating counter: forwards every request to the inner allocator, tallying the ones that hand
/// back fresh capacity.
struct Counting<A>(A);

/// Bump the tally. Allocates nothing itself — a `thread_local!` over a `Cell<u64>` needs no lazy
/// heap init — so it cannot re-enter the allocator.
fn tally() {
    THREAD_ALLOCATIONS.with(|count| count.set(count.get() + 1));
}

// SAFETY: every method forwards to the inner allocator with the pointer and layout it was handed,
// so the allocator contract is whatever the delegate upholds. The tally runs beside the forward and
// touches no allocator state.
unsafe impl<A: GlobalAlloc> GlobalAlloc for Counting<A> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        tally();
        unsafe { self.0.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { self.0.dealloc(pointer, layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        tally();
        unsafe { self.0.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        tally();
        unsafe { self.0.realloc(pointer, layout, new_size) }
    }
}

#[global_allocator]
static COUNTING_ALLOCATOR: Counting<System> = Counting(System);
