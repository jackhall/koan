//! Allocation baselines for the two recorded execute-path shapes.
//!
//! `audit/shapes/tail_loop.koan` is step churn — a tail-recursive countdown, one machine
//! step per iteration. `audit/shapes/operator_chain.koan` is per-dispatch churn — a flat
//! 128-operand `+` chain, one dispatch per operator. Between them they cover the two
//! places the execute path's allocation traffic scales: per step and per dispatch. A
//! re-introduced allocation on either path fails a test here rather than going unnoticed.
//!
//! This test binary installs the same counter the binary's `alloc-count` feature does
//! (`audit/counting_alloc.rs`), so it needs no feature flag and stays in the default
//! verify slate. It reads the counter's **thread-local** tally rather than its
//! process-wide one: the two tests below run concurrently in this one binary, so a shared
//! counter would tally each into the other's bracket. The whole-program figure — the same
//! run plus interpreter startup, read off the process tally — is what `audit/measure.sh`
//! reports and what `audit/README.md` records.
//!
//! Both figures are debug profile, matching every other measurement in the repo, and both
//! are order-independent: [`allocations_for`] warms the process-wide lazy statics before it
//! opens the bracket, so a shape reads the same whether its test runs first, last, or alone.

use std::io;

use koan::machine::interpret_with_writer_path;

#[path = "../audit/counting_alloc.rs"]
mod counting_alloc;

#[global_allocator]
static COUNTING_ALLOCATOR: counting_alloc::Counting<std::alloc::System> =
    counting_alloc::Counting(std::alloc::System);

/// Interpret `source` end to end with its output discarded, returning the allocations the
/// run made. The `Box` around the sink is inside the bracket and so inside the count — one
/// allocation, constant across shapes and across any change to the execute path.
///
/// `path` is passed through rather than defaulted so this reads the same entry point the
/// binary does (`interpret_with_writer_path`, `src/main.rs`). The path-carrying parse
/// allocates more than the `<input>` fallback, in proportion to the source's spans, so
/// defaulting it here would put this bracket and `audit/measure.sh`'s whole-program figure
/// on different call paths.
fn allocations_for(source: &str, path: &str) -> u64 {
    // Run the shape once outside the bracket, to warm every process-wide lazy static it
    // touches. The lexer's `LazyLock<Regex>` (`src/parse/tokens.rs:29`) alone costs ≈1 713
    // allocations, paid once per process by whichever test reaches it first; inside the
    // bracket that lands on whichever test won the race, and a bound tight enough to see one
    // added allocation would be measuring test order instead. Warming with the shape's own
    // source rather than a stand-in is what makes this total: whatever the shape initialises
    // is warm by construction, including statics added later.
    let _ = interpret_with_writer_path(source, Some(path), Box::new(io::sink()));

    let before = counting_alloc::thread_allocations();
    let outcome = interpret_with_writer_path(source, Some(path), Box::new(io::sink()));
    let delta = counting_alloc::thread_allocations() - before;
    assert!(outcome.is_ok(), "shape failed to run: {:?}", outcome.err());
    delta
}

/// 100 tail-recursive steps, 206.0 allocations each — exactly linear, measured flat at
/// 10/50/100/200. Measured 2026-08-18 at 23 808; the bound is that plus 41, less than the 100
/// a single new per-step allocation would add. Tight on purpose: a looser bound cannot see
/// one allocation, and rebaselining is meant to be a deliberate edit.
#[test]
fn the_tail_loop_shape_stays_within_its_step_churn_bound() {
    const BOUND: u64 = 23_849;
    let delta = allocations_for(
        include_str!("../audit/shapes/tail_loop.koan"),
        "audit/shapes/tail_loop.koan",
    );
    assert!(
        delta <= BOUND,
        "the 100-step tail loop allocated {delta} times, over its {BOUND} bound — an \
         allocation was added to the per-step path; re-measure with `audit/measure.sh` \
         and rebaseline deliberately if the cost is intended"
    );
}

/// A 128-operand `+` chain, so 127 dispatches at ≈55 allocations each — mildly superlinear,
/// with marginal cost rising 53.9 → 56.3 across the 16→32 … 128→256 operand doublings (below
/// 16 operands it is flat near 53.3). Measured 2026-08-18 at
/// 9 869; the bound is that plus 41, under the 127 a single new per-dispatch allocation
/// would add. Same headroom rule as the loop.
#[test]
fn the_operator_chain_shape_stays_within_its_dispatch_churn_bound() {
    const BOUND: u64 = 9_910;
    let delta = allocations_for(
        include_str!("../audit/shapes/operator_chain.koan"),
        "audit/shapes/operator_chain.koan",
    );
    assert!(
        delta <= BOUND,
        "the 128-operand chain allocated {delta} times, over its {BOUND} bound — an \
         allocation was added to the per-dispatch path; re-measure with \
         `audit/measure.sh` and rebaseline deliberately if the cost is intended"
    );
}
