//! Allocation baselines for the recorded execute-path shapes.
//!
//! `audit/shapes/tail_loop_steps100.koan` is step churn — a tail-recursive countdown, one
//! machine step per iteration. `audit/shapes/operator_chain_operands128.koan` is per-dispatch
//! churn — a flat 128-operand `+` chain, one dispatch per operator. Between them they cover the two
//! places the execute path's allocation traffic scales, and each is held to an absolute
//! bound. The four `audit/shapes/scope_walk_*.koan` shapes cover a third axis — how far
//! the dispatch scope walk reaches — and are held to a *shape* instead: differenced against
//! each other, so the test pins depth-independence rather than a count. The two
//! `audit/shapes/builtin_call_calls*.koan` shapes cover a fourth — per-call cost at an arity
//! above the binary operator — and are differenced the same way, for an absolute per-call
//! figure. Two further tail-loop variants — `audit/shapes/leading_loop_steps*.koan`, whose FN
//! body carries a leading `LET`, and `audit/shapes/try_loop_steps*.koan`, whose step wraps its
//! recursive argument in a `TRY` — hold the multi-statement-body and catch paths to absolute
//! bounds of their own. A re-introduced allocation on any of these axes fails a test here rather
//! than going unnoticed.
//!
//! This test binary installs the same counter the binary's `alloc-count` feature does
//! (`audit/counting_alloc.rs`), so it needs no feature flag and stays in the default
//! verify slate. It reads the counter's **thread-local** tally rather than its
//! process-wide one: the tests below run concurrently in this one binary, so a shared
//! counter would tally each into the other's bracket. The whole-program figure — the same
//! run plus interpreter startup, read off the process tally — is what `tools/alloc_audit.py`
//! sweeps, and both readings are recorded per shape in `observe/alloc/`.
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
/// defaulting it here would put this bracket and the sweep's whole-program figure
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

    // The bracketed figure, for `tools/alloc_audit.py` to read back under
    // `--nocapture` — captured and so invisible in an ordinary run. It is printed
    // before any bound is asserted, so a bound that fails still reports the
    // measurement a rebaseline is read off.
    println!("bracketed {path} {delta}");
    delta
}

/// 100 tail-recursive steps, at the `step` term in `observe/alloc/terms.txt` — exactly linear,
/// flat across the sizes swept. Two allocations per step are arena chunks rather than heap objects:
/// a step mints two regions, and each takes its whole residency in the one chunk
/// workgraph's `FIRST_CHUNK_BYTES` sizes for it. That pair is flat in the byte size of what a frame
/// holds, because the chunk is sized well clear of the measured spread — a layout change moves this
/// term only by changing how many objects a step allocates, not how large they are.
/// The constant term is the run's seeding, so registering a builtin overload moves this and every
/// other shape here by the same amount. The bound sits over the recorded bracketed reading by less
/// than the 100 a single new per-step allocation would add. Tight on purpose: a looser bound cannot
/// see one allocation, and rebaselining is meant to be a deliberate edit.
#[test]
fn the_tail_loop_shape_stays_within_its_step_churn_bound() {
    const BOUND: u64 = 5_390;
    let delta = allocations_for(
        include_str!("../audit/shapes/tail_loop_steps100.koan"),
        "audit/shapes/tail_loop_steps100.koan",
    );
    assert!(
        delta <= BOUND,
        "the 100-step tail loop allocated {delta} times, over its {BOUND} bound — an \
         allocation was added to the per-step path; re-measure with \
         `tools/alloc_audit.py` and rebaseline deliberately if the cost is intended"
    );
}

/// 100 tail-recursive steps whose FN body carries a leading `LET` before its `MATCH`, at the
/// `leading_loop` term in `observe/alloc/terms.txt`. Against the plain tail loop above it adds
/// what a multi-statement body costs per step: the statement split, the leading-statements
/// finish the wake reads back, and the effect the `LET` deposits. The bound sits over the
/// recorded bracketed reading by less than the 100 a single new per-step allocation would add,
/// same headroom rule as the loop.
#[test]
fn the_leading_loop_shape_stays_within_its_step_churn_bound() {
    const BOUND: u64 = 6_230;
    let delta = allocations_for(
        include_str!("../audit/shapes/leading_loop_steps100.koan"),
        "audit/shapes/leading_loop_steps100.koan",
    );
    assert!(
        delta <= BOUND,
        "the 100-step leading-statement loop allocated {delta} times, over its {BOUND} bound \
         — an allocation was added to the multi-statement body path; re-measure with \
         `tools/alloc_audit.py` and rebaseline deliberately if the cost is intended"
    );
}

/// 100 tail-recursive steps each carrying a `TRY` around the recursive call's argument, at the
/// `try_loop` term in `observe/alloc/terms.txt`. Against the plain tail loop above it adds what
/// a watched sub-expression costs per step: the catch finish the `TRY` parks under and the
/// extra arm the `WITH` selects through. The bound sits over the recorded bracketed reading by
/// less than the 100 a single new per-step allocation would add.
#[test]
fn the_try_loop_shape_stays_within_its_step_churn_bound() {
    const BOUND: u64 = 6_610;
    let delta = allocations_for(
        include_str!("../audit/shapes/try_loop_steps100.koan"),
        "audit/shapes/try_loop_steps100.koan",
    );
    assert!(
        delta <= BOUND,
        "the 100-step TRY loop allocated {delta} times, over its {BOUND} bound — an \
         allocation was added to the catch path; re-measure with \
         `tools/alloc_audit.py` and rebaseline deliberately if the cost is intended"
    );
}

/// A 128-operand `+` chain, so 127 dispatches, at the `dispatch` term in
/// `observe/alloc/terms.txt` — mildly superlinear, with marginal cost rising across the 16→32 …
/// 128→256 operand doublings. A `+` chain reaches no ATTR overload, so the per-dispatch term is
/// unmoved by one being registered; what such a registration moves is the startup constant every
/// shape here carries. The bound sits over the recorded bracketed reading by less than the 127 a
/// single new per-dispatch allocation would add. Same headroom rule as the loop.
#[test]
fn the_operator_chain_shape_stays_within_its_dispatch_churn_bound() {
    const BOUND: u64 = 2_900;
    let delta = allocations_for(
        include_str!("../audit/shapes/operator_chain_operands128.koan"),
        "audit/shapes/operator_chain_operands128.koan",
    );
    assert!(
        delta <= BOUND,
        "the 128-operand chain allocated {delta} times, over its {BOUND} bound — an \
         allocation was added to the per-dispatch path; re-measure with \
         `tools/alloc_audit.py` and rebaseline deliberately if the cost is intended"
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
/// dispatch — the `scope_walk_scope` term in `observe/alloc/terms.txt`, which reads as zero:
/// the two depths are indistinguishable. The walk's per-scope buffers are hosted on the drain's
/// step scratch arena, which is what took that term to zero and is where a regression would put
/// it back.
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

/// **Per-call cost of a builtin call, at an arity the operator chain does not reach.** The two
/// `audit/shapes/builtin_call_calls*.koan` shapes are 8 and 40 repetitions of one
/// three-parameter builtin call (`MATCH … -> … WITH …`, bound as `expr` / `return_type` /
/// `branches`). Differencing them cancels interpreter startup and leaves 32 calls' marginal
/// cost — the parse of the 32 extra statements included, since that is how the shapes differ.
///
/// The recorded figure is the `builtin_call` term in `observe/alloc/terms.txt`. What a call no
/// longer pays at this arity is the 2n = 6 parameter-name copies a three-parameter bind used to
/// make, plus the two per-call containers — the argument map and the carrier map — that the
/// schema-keyed argument view replaces with a slice on the step scratch arena, plus the one name
/// copy per call a symbol-keyed scope binding table no longer makes.
///
/// The bound sits over the recorded reading by under 32 — the repetition gap the shapes
/// difference by — so one re-introduced per-call allocation fails it.
#[test]
fn the_builtin_call_shape_stays_within_its_per_call_bound() {
    const BOUND: u64 = 1_425;
    let marginal = allocations_for(
        include_str!("../audit/shapes/builtin_call_calls40.koan"),
        "audit/shapes/builtin_call_calls40.koan",
    ) - allocations_for(
        include_str!("../audit/shapes/builtin_call_calls8.koan"),
        "audit/shapes/builtin_call_calls8.koan",
    );
    assert!(
        marginal <= BOUND,
        "32 three-parameter builtin calls allocated {marginal} times, over the {BOUND} bound \
         — a per-call or per-parameter allocation is back on the builtin bind path; re-measure \
         with `tools/alloc_audit.py` and rebaseline deliberately if the cost is intended"
    );
}

/// **Per-call cost of a *user-defined* function call, and its slope in the parameter count.**
/// The four `audit/shapes/user_fn_params*.koan` shapes are a parameter-count × call-count grid:
/// a one-parameter and an eight-parameter `FN`, each called 8 and 40 times. Differencing the two
/// call counts at one arity cancels interpreter startup and the definition itself, leaving 32
/// calls' marginal cost; differencing *those* leaves what seven extra parameters cost per call.
///
/// This is the frame bind's own axis. A user call binds each parameter into the fresh per-call
/// scope, and the name it binds under comes straight off the signature's parameter schema — which
/// carries the classified symbol the binding table keys by, so the bind reaches no interner and
/// builds no string. Nor does the call's declared-return contract: it seals a `Copy` call site and
/// the callable's interned type handle, and renders trace text only on the error arm that spends
/// it. What remains in the slope is per-*argument* cost the bind does not own: the extra source
/// the call site parses, and the delivery carrier each argument travels in.
///
/// The recorded figures are the `user_fn_params1`, `user_fn_params8` and `user_fn_parameter`
/// terms in `observe/alloc/terms.txt`. About one allocation per call at eight parameters is an
/// arena chunk, not heap traffic: at that arity the call's own region does not fit bumpalo's
/// 496-byte first chunk and takes a second one, while at one parameter it is never near the
/// boundary. That term follows the byte size of what a frame holds and flips with any layout
/// change either way.
///
/// Both bounds sit over their recorded readings by under one repetition gap, so one re-introduced
/// per-call allocation — 32 across the gap — fails the first, and a re-introduced per-parameter
/// one fails the second by ≈224.
#[test]
fn the_user_fn_call_shape_stays_within_its_per_parameter_bound() {
    const PER_CALL_BOUND: u64 = 655;
    const PER_PARAMETER_BOUND: u64 = 381;
    let arity1 = allocations_for(
        include_str!("../audit/shapes/user_fn_params1_calls40.koan"),
        "audit/shapes/user_fn_params1_calls40.koan",
    ) - allocations_for(
        include_str!("../audit/shapes/user_fn_params1_calls8.koan"),
        "audit/shapes/user_fn_params1_calls8.koan",
    );
    let arity8 = allocations_for(
        include_str!("../audit/shapes/user_fn_params8_calls40.koan"),
        "audit/shapes/user_fn_params8_calls40.koan",
    ) - allocations_for(
        include_str!("../audit/shapes/user_fn_params8_calls8.koan"),
        "audit/shapes/user_fn_params8_calls8.koan",
    );
    assert!(
        arity1 <= PER_CALL_BOUND,
        "32 one-parameter user calls allocated {arity1} times, over the {PER_CALL_BOUND} bound \
         — an allocation was added to the per-call frame bind; re-measure with \
         `tools/alloc_audit.py` and rebaseline deliberately if the cost is intended"
    );
    let slope = arity8 - arity1;
    assert!(
        slope <= PER_PARAMETER_BOUND,
        "seven extra parameters cost {slope} allocations over 32 calls ({arity8} at arity 8 \
         against {arity1} at arity 1), over the {PER_PARAMETER_BOUND} bound — the frame bind is \
         building something per parameter again; the schema's own symbol is what it binds under, \
         so it should reach neither the interner nor the heap"
    );
}

/// **Per-construction cost of a tagged-union value, and the type-registry reads behind it.**
/// The two `audit/shapes/tagged_construct_calls*.koan` shapes are 8 and 40 repetitions of one
/// `MATCH (Maybe (Some 1)) -> :Number WITH (…)` statement over a two-variant `UNION`.
/// Differencing them cancels interpreter startup and the declaration itself, leaving 32
/// construct-and-match cycles' marginal cost — the parse of the 32 extra statements included,
/// since that is how the shapes differ.
///
/// This is the nominal-member axis. Each cycle probes the union's members through the registry —
/// a by-reference walk under one borrow that hands back the variant and its payload type by value,
/// no node clone — builds the tagged value, and matches on its tag.
///
/// The recorded figure is the `tagged_construct` term in `observe/alloc/terms.txt`. One
/// allocation per cycle is an arena chunk rather than heap traffic: the cycle's frame does not fit
/// bumpalo's 496-byte first chunk, so its region takes a second one. That term follows the byte
/// size of what a frame holds and flips with any layout change either way.
///
/// The bound sits over the recorded reading by under 32 — the repetition gap — so one
/// re-introduced per-construction allocation fails it.
#[test]
fn the_tagged_construct_shape_stays_within_its_per_construction_bound() {
    const BOUND: u64 = 1_775;
    let marginal = allocations_for(
        include_str!("../audit/shapes/tagged_construct_calls40.koan"),
        "audit/shapes/tagged_construct_calls40.koan",
    ) - allocations_for(
        include_str!("../audit/shapes/tagged_construct_calls8.koan"),
        "audit/shapes/tagged_construct_calls8.koan",
    );
    assert!(
        marginal <= BOUND,
        "32 tagged constructions allocated {marginal} times, over the {BOUND} bound — an \
         allocation was added to the tagged-construction path; re-measure with \
         `tools/alloc_audit.py` and rebaseline deliberately if the cost is intended"
    );
}
