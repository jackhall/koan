//! Crate-wide test scaffolding.
//!
//! Installs the delegating counter from [`audit/counting_alloc.rs`](../../audit/counting_alloc.rs)
//! over the system allocator, for this crate's own unit-test binary only. An allocation count
//! states a heap contract without drifting with the host: [`allocation_count`] reads the counter's
//! thread-local tally, so a test brackets the call it is measuring and asserts the delta.
//!
//! The tally is thread-local because the test harness runs tests concurrently, so a shared
//! counter would tally every other test's allocations into the bracket.

#[path = "../../audit/counting_alloc.rs"]
mod counting_alloc;

/// The number of heap allocations this thread has made since it started.
pub(crate) fn allocation_count() -> u64 {
    counting_alloc::thread_allocations()
}

#[global_allocator]
static COUNTING_ALLOCATOR: counting_alloc::Counting<std::alloc::System> =
    counting_alloc::Counting(std::alloc::System);

/// The case count for one property-test module, as a share of the proptest default.
///
/// `ProptestConfig::default().cases` reads `PROPTEST_CASES`, so a verification tier sets that one
/// variable and every module moves with it. A module states its share of the default rather than a
/// literal — `case_share(1, 1)` for the lattice's binary laws — so their relative depths hold at
/// whatever the tier asks for. No module drops below 64 cases: a sweep shallower than that is not
/// worth the build it rides on.
pub(crate) fn case_share(numerator: u32, denominator: u32) -> u32 {
    (proptest::prelude::ProptestConfig::default().cases * numerator / denominator).max(64)
}
