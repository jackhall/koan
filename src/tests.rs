//! Crate-wide test scaffolding.
//!
//! A counting wrapper over the system allocator, installed for the library's own unit-test binary
//! only. The relocation path's fixed cost at small N is an acceptance criterion of the N-ary
//! relocation door, and an allocation count is the only way to state it that does not drift with
//! the machine: [`allocation_count`] reads a thread-local tally, so a test brackets the call it is
//! measuring and asserts the delta.
//!
//! Thread-local rather than global: the test harness runs tests concurrently, so a shared counter
//! would tally every other test's allocations into the bracket.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static ALLOCATIONS: Cell<u64> = const { Cell::new(0) };
}

/// The number of heap allocations this thread has made since it started.
pub(crate) fn allocation_count() -> u64 {
    ALLOCATIONS.with(Cell::get)
}

struct Counting;

// SAFETY: every method forwards to `System` with the layout it was handed, so the allocator
// contract is `System`'s. The tally is a thread-local `Cell` bump on the way through, which
// allocates nothing itself — a `thread_local!` over a `Cell<u64>` needs no lazy heap init — so it
// cannot re-enter the allocator.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.with(|count| count.set(count.get() + 1));
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCATIONS.with(|count| count.set(count.get() + 1));
        unsafe { System.realloc(pointer, layout, new_size) }
    }
}

#[global_allocator]
static COUNTING_ALLOCATOR: Counting = Counting;
