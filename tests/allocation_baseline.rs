//! Allocation baselines for the recorded execute-path shapes.
//!
//! `audit/shapes/tail_loop.koan` is step churn — a tail-recursive countdown, one machine
//! step per iteration. `audit/shapes/operator_chain.koan` is per-dispatch churn — a flat
//! 128-operand `+` chain, one dispatch per operator. Between them they cover the two
//! places the execute path's allocation traffic scales, and each is held to an absolute
//! bound. The four `audit/shapes/scope_walk_*.koan` shapes cover a third axis — how far
//! the dispatch scope walk reaches — and are held to a *shape* instead: differenced against
//! each other, so the test pins depth-independence rather than a count. A re-introduced
//! allocation on any of the three fails a test here rather than going unnoticed.
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
    // touches. One-time state paid by whichever test reaches it first would otherwise land
    // inside that test's bracket, and a bound tight enough to see one added allocation would
    // be measuring test order instead. The interpreter currently holds almost none — the
    // warm-up absorbs ≈1 allocation — but the bounds below are tight enough that a single
    // lazy static added later would break them by order alone. Warming with the shape's own
    // source rather than a stand-in is what keeps that coverage total: whatever the shape
    // initialises is warm by construction, including statics added later.
    let _ = interpret_with_writer_path(source, Some(path), Box::new(io::sink()));

    let before = counting_alloc::thread_allocations();
    let outcome = interpret_with_writer_path(source, Some(path), Box::new(io::sink()));
    let delta = counting_alloc::thread_allocations() - before;
    assert!(outcome.is_ok(), "shape failed to run: {:?}", outcome.err());
    delta
}

/// 100 tail-recursive steps, 118.0 allocations each — exactly linear, measured flat at
/// 10/50/100/200. Measured 2026-08-20 at 14 917; the bound is that plus 41, less than the 100
/// a single new per-step allocation would add. Tight on purpose: a looser bound cannot see
/// one allocation, and rebaselining is meant to be a deliberate edit.
#[test]
fn the_tail_loop_shape_stays_within_its_step_churn_bound() {
    const BOUND: u64 = 14_958;
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

/// A 128-operand `+` chain, so 127 dispatches at ≈30 allocations each — mildly superlinear,
/// with marginal cost rising 28.9 → 31.3 across the 16→32 … 128→256 operand doublings.
/// Measured 2026-08-20 at 6 689; the bound is that plus 41, under the 127 a single new
/// per-dispatch allocation would add. Same headroom rule as the loop.
#[test]
fn the_operator_chain_shape_stays_within_its_dispatch_churn_bound() {
    const BOUND: u64 = 6_730;
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

/// **Per-dispatch cost is independent of how deep the scope walk is.** The four
/// `audit/shapes/scope_walk_*.koan` shapes are a depth × call-count grid: at each depth, an
/// innermost body of *m* `PROBE y` statements under *n* nested scopes that each shadow the
/// `PROBE` bucket with a non-admitting same-key overload, so every dispatch strict- and
/// hard-rejects at all *n* shadow scopes before picking at the root.
///
/// Differencing the two call counts at one depth cancels parse and setup, leaving 32
/// dispatches' marginal cost; differencing *those* leaves what 8 extra scopes cost per
/// dispatch. Before the walk's buffers moved onto the step scratch arena that difference
/// measured 509 — ≈2 heap allocations per extra scope walked, per dispatch. Measured
/// 2026-08-20 it is **−3**: 1 883 allocations for 32 dispatches at depth 10 against 1 886 at
/// depth 2, the two depths indistinguishable and the deeper walk marginally the cheaper.
///
/// The bound is one allocation per extra dispatch, far under the ≥256 that a single
/// reintroduced per-scope allocation would add (8 extra scopes × 32 dispatches).
#[test]
fn per_dispatch_cost_does_not_grow_with_scope_walk_depth() {
    const BOUND: u64 = 32;
    let per_dispatch_at_depth10 = allocations_for(
        include_str!("../audit/shapes/scope_walk_depth10_calls40.koan"),
        "audit/shapes/scope_walk_depth10_calls40.koan",
    ) - allocations_for(
        include_str!("../audit/shapes/scope_walk_depth10_calls8.koan"),
        "audit/shapes/scope_walk_depth10_calls8.koan",
    );
    let per_dispatch_at_depth2 = allocations_for(
        include_str!("../audit/shapes/scope_walk_depth2_calls40.koan"),
        "audit/shapes/scope_walk_depth2_calls40.koan",
    ) - allocations_for(
        include_str!("../audit/shapes/scope_walk_depth2_calls8.koan"),
        "audit/shapes/scope_walk_depth2_calls8.koan",
    );
    let growth = per_dispatch_at_depth10.saturating_sub(per_dispatch_at_depth2);
    assert!(
        growth <= BOUND,
        "32 dispatches cost {per_dispatch_at_depth10} allocations at scope depth 10 against \
         {per_dispatch_at_depth2} at depth 2 — {growth} more, over the {BOUND} bound. A \
         per-scope allocation is back on the dispatch walk; find the buffer that left the \
         step scratch arena."
    );
}

