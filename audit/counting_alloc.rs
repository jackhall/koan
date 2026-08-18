//! A counting `GlobalAlloc` that delegates to an inner allocator.
//!
//! Lives outside `src/` because it is measurement scaffolding, not library code: the
//! `unsafe impl` here is not a production site the Miri slate owes a group, and koan's
//! shipped binary never compiles it. Three crates `#[path]`-include this one file —
//! the library's own test build (`src/tests.rs`), the binary under the `alloc-count`
//! feature (`src/main.rs`), and the baseline regression test
//! (`tests/allocation_baseline.rs`) — so there is one wrapper, not one per target.
//!
//! Two tallies, both bumped on the way through:
//!
//! - [`allocations`] reads a process-wide atomic. This is the whole-program number: a
//!   binary's `main` cannot read another thread's thread-local, so a per-thread tally
//!   could not report a program run's total.
//! - [`thread_allocations`] reads a thread-local. This is the bracketing number: the
//!   test harness runs tests concurrently, so a bracket around one call has to be
//!   insulated from every other test's traffic.

// Each including target uses a different part of this surface — the binary reads the
// process tally, the tests read the thread one — so an unused reader is expected rather
// than dead.
#![allow(dead_code)]

use std::alloc::{GlobalAlloc, Layout};
use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};

static PROCESS_ALLOCATIONS: AtomicU64 = AtomicU64::new(0);

thread_local! {
    static THREAD_ALLOCATIONS: Cell<u64> = const { Cell::new(0) };
}

/// The number of heap allocations this process has made since it started.
pub fn allocations() -> u64 {
    PROCESS_ALLOCATIONS.load(Ordering::Relaxed)
}

/// The number of heap allocations the calling thread has made since it started.
pub fn thread_allocations() -> u64 {
    THREAD_ALLOCATIONS.with(Cell::get)
}

/// Delegating counter: forwards every request to `A`, tallying the ones that hand back
/// fresh capacity. Wrapping rather than replacing is what keeps a counted build and a
/// shipped build on the same allocator, so a wall-clock reading off the counted one is
/// still comparable.
pub struct Counting<A>(pub A);

/// Bump both tallies. Allocates nothing itself — a `thread_local!` over a `Cell<u64>` needs
/// no lazy heap init — so it cannot re-enter the allocator.
fn tally() {
    PROCESS_ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
    THREAD_ALLOCATIONS.with(|count| count.set(count.get() + 1));
}

// SAFETY: every method forwards to the inner allocator with the pointer and layout it was
// handed, so the allocator contract is whatever `A` upholds. The tally runs beside the
// forward and touches no allocator state.
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
