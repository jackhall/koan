//! Allocation baselines for the recorded execute-path shapes.
//!
//! Four shapes, each with a single scaling parameter `n`, each held to one bound.
//! `audit/shapes/empty.koan` is the fixed cost every other shape carries — interpreter
//! startup and builtin seeding, and nothing else. The other three are two files apiece
//! that differ in `n` alone, so differencing a pair cancels startup and the shape's own
//! parse and declarations exactly, leaving the marginal cost of one unit of `n`.
//!
//! `audit/shapes/wide_n{10,100}.koan` scale **steps**: one tail-recursive driver whose
//! body reaches across the runtime in a single iteration — a record construct and field
//! read, a module opened with `USING`, a tagged-union construct and `MATCH`, an operator
//! chain, an overloaded user call, a record projection through `FROM`, a `TRY` and a
//! `CATCH`, a quote and its evaluation, a `NEWTYPE`, `TYPE OF`, list and map literals,
//! and an anonymous function applied by named argument. TCO holds the node table and the
//! regions flat, so the term is per-*step* churn.
//!
//! `audit/shapes/deep_n{10,100}.koan` scale **live frames**: the same body with the
//! recursive call out of tail position, so each iteration's frame is still live when the
//! next one runs. This is the axis the wide shape structurally cannot see, and the one
//! place a cost that grows with the number of standing frames shows up.
//!
//! `audit/shapes/declare_n{10,100}.koan` scale **declared names**: a `UNION`, a record
//! `NEWTYPE`, a `SIG`, a `MODULE` and an `FN` signature, each carrying `n` names. Nothing
//! runs, so this is the declaration and registration side alone — the axis the two
//! recursion-driven shapes hold constant, and the one the symbol-mint column reads.
//!
//! This test binary installs the same counter the binary's `alloc-count` feature does
//! (`audit/counting_alloc.rs`), so it needs no feature flag and stays in the default
//! verify slate. It reads the counter's **thread-local** tally rather than its
//! process-wide one: the tests below run concurrently in this one binary, so a shared
//! counter would tally each into the other's bracket. The whole-program figure — the same
//! run plus interpreter startup, read off the process tally — is what `tools/alloc_audit.py`
//! sweeps, and both readings are swept together, the whole-program one recorded in `observe/alloc.txt`.
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

/// The empty program: interpreter startup and builtin seeding, which every other shape
/// here carries. It is the `fixed` term in `observe/alloc.txt`, and it is inert to
/// what a program *names* — an empty program names nothing. What moves it is the *count*
/// of registered overloads, so registering a builtin moves this bound and every absolute
/// figure in the record with it.
///
/// Its headroom rule is unlike the marginal shapes': nothing repeats here, so one added
/// allocation adds exactly one. The bound is set a little over the reading, tight enough
/// that a seeding change of any real size fails it rather than being absorbed.
#[test]
fn the_empty_program_stays_within_its_startup_bound() {
    const BOUND: u64 = 1_060;
    let delta = allocations_for(
        include_str!("../audit/shapes/empty.koan"),
        "audit/shapes/empty.koan",
    );
    assert!(
        delta <= BOUND,
        "the empty program allocated {delta} times, over its {BOUND} bound — interpreter \
         startup or builtin seeding got more expensive; re-measure with \
         `tools/alloc_audit.py` and rebaseline deliberately if the cost is intended"
    );
}

/// 100 tail-recursive steps through the wide body, at the `wide_step` term in
/// `observe/alloc.txt` — flat across the sizes swept, so the shape is exactly linear
/// in `n`. TCO holds the node table and the regions flat, so what this prices is per-step
/// churn rather than anything that accumulates: a step mints one region and takes its whole
/// residency in the single chunk workgraph's `FIRST_CHUNK_BYTES` sizes for it.
///
/// The body is deliberately broad rather than isolating one path, so a regression on any of
/// the execute path's marginal costs lands here. It says *that* something moved; the `dhat`
/// cargo feature and `tools/dhat_diff.py` say *where* (see `audit/README.md`).
///
/// The bound sits over the recorded bracketed reading by less than the 100 a single new
/// per-step allocation would add. Tight on purpose: a looser bound cannot see one
/// allocation, and rebaselining is meant to be a deliberate edit.
#[test]
fn the_wide_shape_stays_within_its_per_step_bound() {
    const BOUND: u64 = 22_760;
    let delta = allocations_for(
        include_str!("../audit/shapes/wide_n100.koan"),
        "audit/shapes/wide_n100.koan",
    );
    assert!(
        delta <= BOUND,
        "the 100-step wide shape allocated {delta} times, over its {BOUND} bound — an \
         allocation was added to a per-step path; re-measure with `tools/alloc_audit.py`, \
         attribute it with the `dhat` feature, and rebaseline deliberately if intended"
    );
}

/// The same body at recursion depth 100, with the recursive call out of tail position, at
/// the `deep_frame` term in `observe/alloc.txt`. Every frame is still live when the
/// next one runs, so this is the only shape here that prices what standing frames cost.
///
/// The recorded term is the cost averaged over depth 10 to 100, and it tracks the wide shape's
/// per-step figure: no path that runs once per step walks or hashes anything whose size is the
/// number of standing frames. A term that pulls away from `wide_step` is this shape's own
/// signal — a cost that only depth can see.
///
/// The bound sits over the recorded bracketed reading by less than the 100 a single new
/// per-frame allocation would add.
#[test]
fn the_deep_shape_stays_within_its_per_frame_bound() {
    const BOUND: u64 = 23_540;
    let delta = allocations_for(
        include_str!("../audit/shapes/deep_n100.koan"),
        "audit/shapes/deep_n100.koan",
    );
    assert!(
        delta <= BOUND,
        "the depth-100 shape allocated {delta} times, over its {BOUND} bound — an \
         allocation was added to a path that runs per live frame; re-measure with \
         `tools/alloc_audit.py` and rebaseline deliberately if the cost is intended"
    );
}

/// **Per-declared-name cost of the declaration side.** The two
/// `audit/shapes/declare_n{10,100}.koan` shapes carry `n` names in each of five declaration
/// forms — a `UNION`'s variants, a record `NEWTYPE`'s fields, a `SIG`'s `VAL` members, a
/// `MODULE`'s bindings, and an `FN` signature's parameters. Nothing in either shape runs, so
/// differencing them leaves registration alone: the schemas built, the buckets keyed, and the
/// symbols minted, with the parse of the extra names included since that is how they differ.
///
/// This is the axis the two recursion-driven shapes hold constant — their `n` scales work
/// done, not names declared — and it is where the symbols column has a marginal term to read
/// at all. Registering a callable renders no signature text: the `DuplicateOverload`
/// diagnostic renders from the standing entry's stored dispatch token on the error arm, so an
/// entry stores none.
///
/// The recorded figure is the `declare_name` term in `observe/alloc.txt`. The bound is
/// on the difference, and sits over it by less than the 450 that one allocation added per
/// declared name would cost across the 90-name gap in five forms.
#[test]
fn the_declare_shape_stays_within_its_per_name_bound() {
    const BOUND: u64 = 2_710;
    let marginal = allocations_for(
        include_str!("../audit/shapes/declare_n100.koan"),
        "audit/shapes/declare_n100.koan",
    ) - allocations_for(
        include_str!("../audit/shapes/declare_n10.koan"),
        "audit/shapes/declare_n10.koan",
    );
    assert!(
        marginal <= BOUND,
        "90 extra declared names in five forms allocated {marginal} times, over the {BOUND} \
         bound — an allocation was added to the declaration path; re-measure with \
         `tools/alloc_audit.py` and rebaseline deliberately if the cost is intended"
    );
}
