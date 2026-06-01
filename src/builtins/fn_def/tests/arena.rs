//! Run-root arena and scheduler-slot reclamation invariants for user FN calls.

use crate::builtins::test_support::{parse_one, run, run_one, run_root_silent, run_root_with_buf};
use crate::machine::execute::Scheduler;
use crate::machine::RuntimeArena;

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
        "tail-call slot reuse = AA -> BB -> PRINT should collapse into one slot, got {}",
        sched.len(),
    );
}

#[test]
fn chained_tail_calls_reuse_frames() {
    let arena = RuntimeArena::new();
    let (scope, captured) = run_root_with_buf(&arena);

    run(
        scope,
        "FN (DD) -> Null = (PRINT \"ok\")\n\
         FN (CC) -> Null = (DD)\n\
         FN (BB) -> Null = (CC)\n\
         FN (AA) -> Null = (BB)",
    );

    let mut sched = Scheduler::new();
    sched.add_dispatch(parse_one("AA"), scope);
    sched.execute().expect("AA should run");

    assert_eq!(captured.borrow().as_slice(), b"ok\n");
    assert_eq!(sched.len(), 1, "tail chain should collapse to one slot");
    assert!(
        sched.tail_reuse_count() >= 3,
        "expected at least 3 reuses across AA -> BB -> CC -> DD, got {}",
        sched.tail_reuse_count(),
    );
}

/// Recursive tail-call through a `MATCH` arm. Pins the refcount-driven reuse
/// refusal one step out, resume one step later; see
/// [per-call-arena-protocol.md § MATCH frame lifetime under tail recursion](../../../../design/per-call-arena-protocol.md#match-frame-lifetime-under-tail-recursion).
#[test]
fn match_driven_tail_recursion_completes() {
    let arena = RuntimeArena::new();
    let (scope, captured) = run_root_with_buf(&arena);

    run(
        scope,
        "UNION Bit = (one :Null zero :Null)\n\
         FN (HOP b :Tagged) -> Any = (MATCH (b) WITH (\
             one -> (HOP (Bit (zero null)))\
             zero -> (PRINT \"done\")\
         ))",
    );

    let mut sched = Scheduler::new();
    sched.add_dispatch(parse_one("HOP (Bit (one null))"), scope);
    sched.execute().expect("HOP should run");

    assert_eq!(captured.borrow().as_slice(), b"done\n");
}
#[test]
fn repeated_user_fn_calls_do_not_grow_run_root_per_call() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(scope, "FN (ECHO v :Number) -> Number = (v)");
    let baseline = arena.alloc_count();
    for _ in 0..50 {
        let _ = run_one(scope, parse_one("ECHO 7"));
    }
    let after = arena.alloc_count();
    let growth = after - baseline;
    // Measured at 50 (one `KObject::Number(7)` lifted per call); < 150 catches
    // any regression that re-introduces a per-call leak into run-root.
    assert!(
        growth < 50 * 3,
        "per-call leak regression: {growth} new run-root allocations across 50 \
         ECHO calls (expected < 150)",
    );
}

/// Property: after a warmup call populates the free-list with the body's
/// transient slots, steady-state per-call growth in `nodes.len()` is bounded
/// by the *persistent* per-call overhead — the top-level dispatch slot plus
/// any persistent shim. Without reclamation, every call would leave its
/// body's transient fanout (~5+ slots/call) behind.
#[test]
fn body_subexpression_slots_recycle_across_calls() {
    let arena = RuntimeArena::new();
    let (scope, captured) = run_root_with_buf(&arena);

    run(
        scope,
        "UNION Bit = (one :Null zero :Null)\n\
         FN (LOOK b :Tagged) -> Any = (MATCH (b) WITH (\
             one -> (PRINT \"one\")\
             zero -> (PRINT \"zero\")\
         ))",
    );

    let mut sched = Scheduler::new();

    // Warmup: populates the free-list with the body's transient pool.
    sched.add_dispatch(parse_one("LOOK (Bit (one null))"), scope);
    sched.execute().expect("LOOK should run");
    let after_warmup = sched.len();

    let n = 30;
    for i in 1..=n {
        let src = if i % 2 == 0 {
            "LOOK (Bit (one null))"
        } else {
            "LOOK (Bit (zero null))"
        };
        sched.add_dispatch(parse_one(src), scope);
        sched.execute().expect("LOOK should run");
    }
    let after_batch = sched.len();

    assert_eq!(
        captured.borrow().iter().filter(|&&b| b == b'\n').count(),
        n + 1,
        "expected one PRINT per LOOK call, got {:?}",
        String::from_utf8_lossy(&captured.borrow()),
    );

    // Bound of 3: steady-state is 2 slots/call (top-level dispatch + Lift shim);
    // a transient leak would push it to ≥ 5.
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
