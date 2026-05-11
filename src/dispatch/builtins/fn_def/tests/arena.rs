//! Run-root arena and scheduler-slot reclamation invariants for user FN calls.

use crate::dispatch::builtins::test_support::{parse_one, run, run_one, run_root_silent, run_root_with_buf};
use crate::dispatch::runtime::RuntimeArena;
use crate::execute::scheduler::Scheduler;

#[test]
fn chained_user_fn_tail_calls_reuse_one_slot() {
    let arena = RuntimeArena::new();
    let (scope, captured) = run_root_with_buf(&arena);

    run(
        scope,
        "FN (BB) -> Null = (PRINT \"ok\")\n\
         FN (AA) -> Null = (BB)",
    );

    let mut sched = Scheduler::new();
    sched.add_dispatch(parse_one("AA"), scope);
    sched.execute().expect("AA should run");

    assert_eq!(captured.borrow().as_slice(), b"ok\n");
    assert_eq!(
        sched.len(),
        1,
        "tail-call slot reuse: AA -> BB -> PRINT should collapse into one slot, got {}",
        sched.len(),
    );
}

/// A parameterized user-fn called many times must not grow the run-root arena per
/// call: per-call allocations (child scope, param clones, body rewrites, value_pass
/// clones) belong in the per-call arena, leaving only the lifted return value in
/// run-root — one `KObject::Number` per call here. The bound (~3 allocations/call)
/// tolerates the lift while catching any future regression that re-introduces a
/// per-call leak into run-root.
#[test]
fn repeated_user_fn_calls_do_not_grow_run_root_per_call() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "FN (ECHO v: Number) -> Number = (v)");
    let baseline = arena.alloc_count();
    for _ in 0..50 {
        let _ = run_one(scope, parse_one("ECHO 7"));
    }
    let after = arena.alloc_count();
    let growth = after - baseline;
    // Measured at exactly 50 (one `KObject::Number(7)` lifted per call). The < 150
    // bound tolerates that and catches any regression that re-introduces a per-call
    // leak into run-root.
    assert!(
        growth < 50 * 3,
        "per-call leak regression: {growth} new run-root allocations across 50 \
         ECHO calls (expected < 150)",
    );
}

/// Repeated calls to a user-fn whose body has an internal sub-expression reuse
/// scheduler slots. The body of `LOOK` evaluates `MATCH (b) WITH …`; the `(b)`
/// is a sub-expression that spawns a sub-`Dispatch` and a parent `Bind` per call.
///
/// Property under test: after a warmup call has populated the free-list with the
/// body's transient slots, each subsequent call's growth in `nodes.len()` is
/// bounded by the *persistent* per-call overhead — the top-level dispatch slot
/// itself, plus any persistent shim it lifts into. The body's transient sub-
/// Dispatches/Binds must be recycled, not accumulated.
///
/// Without reclamation, every call would leave its body's transient fanout
/// behind (~5+ slots/call). With reclamation, the steady-state rate is the
/// persistent overhead alone (a small constant ≤ 2 today). Comparing two
/// batches catches super-linear growth without coupling to the exact constant.
///
/// The truly-recursive variant (where the body tail-calls itself) is exercised
/// by `match_case::tests::recursive_tagged_match_no_uaf`.
#[test]
fn body_subexpression_slots_recycle_across_calls() {
    let arena = RuntimeArena::new();
    let (scope, captured) = run_root_with_buf(&arena);

    run(
        scope,
        "UNION Bit = (one: Null zero: Null)\n\
         FN (LOOK b: Tagged) -> Any = (MATCH (b) WITH (\
             one -> (PRINT \"one\")\
             zero -> (PRINT \"zero\")\
         ))",
    );

    let mut sched = Scheduler::new();

    // One warmup call: extends `nodes` with the persistent top-level slot(s)
    // *and* the body's transient pool. After this call returns, the transients
    // are on the free-list, ready to be recycled by the next call's spawns.
    sched.add_dispatch(parse_one("LOOK (Bit (one null))"), scope);
    sched.execute().expect("LOOK should run");
    let after_warmup = sched.len();

    // Steady-state batch. Each call's body re-spawns the same transient shape;
    // those slots come off the free-list, so `nodes` only grows by the
    // persistent per-call overhead.
    let n = 30;
    for i in 1..=n {
        let src = if i % 2 == 0 { "LOOK (Bit (one null))" } else { "LOOK (Bit (zero null))" };
        sched.add_dispatch(parse_one(src), scope);
        sched.execute().expect("LOOK should run");
    }
    let after_batch = sched.len();

    // Sanity: each call printed once.
    assert_eq!(
        captured.borrow().iter().filter(|&&b| b == b'\n').count(),
        n + 1,
        "expected one PRINT per LOOK call, got {:?}",
        String::from_utf8_lossy(&captured.borrow()),
    );

    // The property: steady-state per-call growth is bounded by persistent
    // overhead. Currently 2 slots/call (top-level dispatch + Lift shim); if
    // reclamation regressed and transients leaked, it would be ≥ 5.
    // The bound of 3 reflects the property, not the exact value — it leaves
    // daylight for one extra persistent slot per call without admitting any
    // amount of transient pile-up.
    let growth = after_batch - after_warmup;
    let per_call = growth as f64 / n as f64;
    assert!(
        per_call <= 3.0,
        "transient-node reclamation regressed: {per_call:.2} slots/call \
         across {n} calls (after {after_warmup}-slot warmup, ended at \
         {after_batch}). Expected ≤ 3 — body's transient sub-Dispatches/\
         Binds should be recycled via the free-list, not accumulating."
    );
}
