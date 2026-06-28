//! Run-root region and scheduler-slot reclamation invariants for user FN calls.

use crate::builtins::test_support::{parse_one, run, run_one, run_root_silent, run_root_with_buf};
use crate::machine::core::FrameStorage;
use crate::machine::execute::KoanRuntime;

#[test]
fn chained_user_fn_tail_calls_reuse_one_slot() {
    let region = FrameStorage::run_root();
    let (scope, captured) = run_root_with_buf(&region);

    run(
        scope,
        "FN (BB) -> Null = (PRINT \"ok\")\n\
         FN (AA) -> Null = (BB)",
    );

    let mut sched = KoanRuntime::new();
    sched.dispatch_in_scope(parse_one("AA"), scope);
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
    let region = FrameStorage::run_root();
    let (scope, captured) = run_root_with_buf(&region);

    run(
        scope,
        "FN (DD) -> Null = (PRINT \"ok\")\n\
         FN (CC) -> Null = (DD)\n\
         FN (BB) -> Null = (CC)\n\
         FN (AA) -> Null = (BB)",
    );

    let mut sched = KoanRuntime::new();
    sched.dispatch_in_scope(parse_one("AA"), scope);
    sched.execute().expect("AA should run");

    assert_eq!(captured.borrow().as_slice(), b"ok\n");
    assert_eq!(sched.len(), 1, "tail chain should collapse to one slot");
    // Reuse draws from the per-slot reserve, which is seeded by the *previous* per-call frame.
    // The top-level→first-FN transition parks the non-dying run frame, which is never reusable,
    // so the first FN frame allocates fresh and reuse kicks in from the third call onward —
    // two reuses across AA -> BB -> CC -> DD. Steady-state tail recursion still ping-pongs two
    // frames with no further allocation.
    assert!(
        sched.tail_reuse_count() >= 2,
        "expected at least 2 reuses across AA -> BB -> CC -> DD, got {}",
        sched.tail_reuse_count(),
    );
}

/// Side-effect ordering across a tail chain whose bodies each open with a value-discarded
/// leading `PRINT`. The leading statements are owned deps the slot parks on, so they run — and
/// finish — strictly before the tail continues: `a, b, c, d` (the leading PRINTs, in call order)
/// then `ok` (DD's body terminal). A fire-and-forget leading would race the tail chain and emit
/// the terminal first (`ok, a, b, c, d`).
#[test]
fn leading_statements_run_before_tail_across_chain() {
    let region = FrameStorage::run_root();
    let (scope, captured) = run_root_with_buf(&region);

    run(
        scope,
        "FN (DD) -> Str = ((PRINT \"d\") (PRINT \"ok\"))\n\
         FN (CC) -> Str = ((PRINT \"c\") (DD))\n\
         FN (BB) -> Str = ((PRINT \"b\") (CC))\n\
         FN (AA) -> Str = ((PRINT \"a\") (BB))",
    );

    let mut sched = KoanRuntime::new();
    sched.dispatch_in_scope(parse_one("AA"), scope);
    sched.execute().expect("AA should run");

    assert_eq!(
        String::from_utf8_lossy(&captured.borrow()),
        "a\nb\nc\nd\nok\n",
        "leading PRINTs must run in call order, each before its tail call continues",
    );
}

/// Tail chain whose bodies each carry a value-discarded leading `PRINT` stays TCO-flat: the
/// leading statements are owned deps that cascade-free as each call resolves, so the per-call
/// frame stays uniquely owned and `try_reset_for_tail` keeps reusing it. The chain peaks at two
/// slots — the tail-replaced main slot plus a single leading-PRINT slot recycled through the
/// free-list across all four calls — and frame reuse still kicks in. Fire-and-forget leading
/// would instead leave one orphan PRINT slot per call aliasing its frame (`sched.len()` would
/// climb to 5) and block reuse (`tail_reuse_count` would stay 0).
#[test]
fn chained_tail_calls_with_leading_stay_tco_flat() {
    let region = FrameStorage::run_root();
    let scope = run_root_silent(&region);

    run(
        scope,
        "FN (DD) -> Str = ((PRINT \"d\") (PRINT \"ok\"))\n\
         FN (CC) -> Str = ((PRINT \"c\") (DD))\n\
         FN (BB) -> Str = ((PRINT \"b\") (CC))\n\
         FN (AA) -> Str = ((PRINT \"a\") (BB))",
    );

    let mut sched = KoanRuntime::new();
    sched.dispatch_in_scope(parse_one("AA"), scope);
    sched.execute().expect("AA should run");

    assert_eq!(
        sched.len(),
        2,
        "leading statements are owned and cascade-free, so each PRINT slot is recycled via the \
         free-list rather than orphaned — the chain peaks at the main slot plus one reused \
         leading slot (a leak would climb to 5), got {}",
        sched.len(),
    );
    assert!(
        sched.tail_reuse_count() >= 2,
        "leading statements cascade-free before each tail continues, so the frame stays unique \
         and reuse still kicks in across AA -> BB -> CC -> DD, got {}",
        sched.tail_reuse_count(),
    );
}

/// Recursive tail-call through a `MATCH` arm. Pins the refcount-driven reuse
/// refusal one step out, resume one step later; see
/// [per-call-region/frames.md § MATCH frame lifetime under tail recursion](../../../../design/per-call-region/frames.md#match-frame-lifetime-under-tail-recursion).
#[test]
fn match_driven_tail_recursion_completes() {
    let region = FrameStorage::run_root();
    let (scope, captured) = run_root_with_buf(&region);

    run(
        scope,
        "UNION Bit = (One :Null Zero :Null)\n\
         FN (HOP b :Any) -> Any = (MATCH (b) -> :Str WITH (\
             One -> (HOP (Bit (Zero null)))\
             Zero -> (PRINT \"done\")\
         ))",
    );

    let mut sched = KoanRuntime::new();
    sched.dispatch_in_scope(parse_one("HOP (Bit (One null))"), scope);
    sched.execute().expect("HOP should run");

    assert_eq!(captured.borrow().as_slice(), b"done\n");
}

/// A MATCH arm whose body opens with a value-discarded leading `PRINT` before a tail-recursive
/// call. The arm runs through the action harness (`branch_walk` mints a `FreshChild` frame and
/// emits an `Action::Tail` carrying the leading statement), so this pins that the harness routes
/// arm-body leading statements through the same owned-dep park: the leading `PRINT` runs before
/// the recursion continues, giving `hop` (the One arm) then `done` (the Zero arm) in order.
#[test]
fn match_arm_leading_statement_runs_before_tail_recursion() {
    let region = FrameStorage::run_root();
    let (scope, captured) = run_root_with_buf(&region);

    run(
        scope,
        "UNION Bit = (One :Null Zero :Null)\n\
         FN (HOP b :Any) -> Any = (MATCH (b) -> :Str WITH (\
             One -> ((PRINT \"hop\") (HOP (Bit (Zero null))))\
             Zero -> (PRINT \"done\")\
         ))",
    );

    let mut sched = KoanRuntime::new();
    sched.dispatch_in_scope(parse_one("HOP (Bit (One null))"), scope);
    sched.execute().expect("HOP should run");

    assert_eq!(
        String::from_utf8_lossy(&captured.borrow()),
        "hop\ndone\n",
        "the One arm's leading PRINT must run before its tail call into the Zero arm",
    );
}
/// The caller of `FF` contracted for `FF`'s declared return type, regardless of what `FF`
/// tail-calls internally. `FF -> Number` whose body tail-calls `GG -> Str` must reject the `Str`
/// result against *`FF`'s* contract — not silently accept it against the tail-most `GG` contract.
/// Pins that a tail chain keeps the **first** caller's return contract.
#[test]
fn tail_call_enforces_first_callers_return_contract() {
    use crate::machine::execute::KoanRuntime;
    use crate::machine::KErrorKind;
    let region = FrameStorage::run_root();
    let scope = run_root_silent(&region);
    run(
        scope,
        "FN (GG) -> Str = (\"hello\")\n\
         FN (FF) -> Number = (GG)",
    );
    let mut sched = KoanRuntime::new();
    let id = sched.dispatch_in_scope(parse_one("FF"), scope);
    sched
        .execute()
        .expect("execute does not surface per-slot errors");
    let err = match sched.read_result(id) {
        Err(e) => e,
        Ok(_) => panic!("FF -> Number tail-calling GG -> Str must fail FF's return contract"),
    };
    assert!(
        matches!(err.kind, KErrorKind::TypeMismatch { ref arg, .. } if arg == "<return>"),
        "expected a <return> TypeMismatch against FF's Number contract, got {err}",
    );
}

/// A tail chain checks **and stamps** its result against the first caller's declared return, not
/// the tail-most callee's. `FF -> :(LIST OF Any)` tail-calls `GG -> :(LIST OF Number)` which returns
/// a `List<Number>`; the result coarsens to `List<Any>` (FF's contract). Under the old tail-most
/// rule it would have kept `List<Number>` (GG's) — so the element type discriminates the two.
#[test]
fn tail_call_stamps_result_against_first_callers_return_contract() {
    use crate::machine::model::{KObject, KType};
    let region = FrameStorage::run_root();
    let scope = run_root_silent(&region);
    run(
        scope,
        "FN (GG) -> :(LIST OF Number) = ([1 2 3])\n\
         FN (FF) -> :(LIST OF Any) = (GG)",
    );
    let result = run_one(scope, parse_one("FF"));
    match result {
        KObject::List(_, elem) => assert!(
            matches!(elem.as_ref(), KType::Any),
            "FF -> (LIST OF Any) must coarsen the tail-chain result to List<Any>, got {:?}",
            elem,
        ),
        other => panic!("expected a List from FF, got {:?}", other.ktype()),
    }
}

#[test]
fn repeated_user_fn_calls_do_not_grow_run_root_per_call() {
    let region = FrameStorage::run_root();
    let scope = run_root_silent(&region);
    run(scope, "FN (ECHO v :Number) -> Number = (v)");
    let baseline = region.region().alloc_count();
    for _ in 0..50 {
        let _ = run_one(scope, parse_one("ECHO 7"));
    }
    let after = region.region().alloc_count();
    let growth = after - baseline;
    // Measured at 50 (one `KObject::Number(7)` per call); < 150 catches
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
    let region = FrameStorage::run_root();
    let (scope, captured) = run_root_with_buf(&region);

    run(
        scope,
        "UNION Bit = (One :Null Zero :Null)\n\
         FN (LOOK b :Any) -> Any = (MATCH (b) -> :Str WITH (\
             One -> (PRINT \"one\")\
             Zero -> (PRINT \"zero\")\
         ))",
    );

    let mut sched = KoanRuntime::new();

    // Warmup: populates the free-list with the body's transient pool.
    sched.dispatch_in_scope(parse_one("LOOK (Bit (One null))"), scope);
    sched.execute().expect("LOOK should run");
    let after_warmup = sched.len();

    let n = 30;
    for i in 1..=n {
        let src = if i % 2 == 0 {
            "LOOK (Bit (One null))"
        } else {
            "LOOK (Bit (Zero null))"
        };
        sched.dispatch_in_scope(parse_one(src), scope);
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

/// A closure capturing a per-call value survives a `let`-bind: `MAKE_HOLDER` returns a closure over
/// its `base` argument, which lives in MAKE_HOLDER's per-call frame; `LET hold` binds the closure,
/// retiring that frame. Calling `hold` reads the captured `base`, so the bind's carrier fold (C1)
/// must keep the producing frame's region alive. (Under Miri this is the no-use-after-free check for
/// a captured per-call value read after its producing frame retires.)
#[test]
fn captured_per_call_value_survives_let_bind_and_call() {
    use crate::machine::model::KObject;
    let region = FrameStorage::run_root();
    let scope = run_root_silent(&region);
    run(
        scope,
        "FN (MAKE_HOLDER base :Number) -> :(FN (q :Number) -> Number) = \
         (FN (GET q :Number) -> Number = (base))\n\
         LET hold = (MAKE_HOLDER 99)",
    );
    let result = run_one(scope, parse_one("hold {q = 0}"));
    assert!(
        matches!(result, KObject::Number(n) if *n == 99.0),
        "the let-bound closure must read its captured base=99, got {:?}",
        result.ktype(),
    );
}

/// A closure passed as a user-fn argument stays live through the call: `CALL_IT` receives a closure
/// over `base` (in MAKE_HOLDER's per-call frame) and invokes it. The arg-bind carrier fold (D1) must
/// keep that frame alive for the per-call scope, so the inner read of `base` does not dangle. (Miri:
/// no-use-after-free for a closure argument invoked inside the callee.)
#[test]
fn closure_argument_stays_live_through_user_fn_call() {
    use crate::machine::model::KObject;
    let region = FrameStorage::run_root();
    let scope = run_root_silent(&region);
    run(
        scope,
        "FN (MAKE_HOLDER base :Number) -> :(FN (q :Number) -> Number) = \
         (FN (GET q :Number) -> Number = (base))\n\
         FN (CALL_IT f :(FN (q :Number) -> Number)) -> Number = (f {q = 0})\n\
         LET answer = (CALL_IT (MAKE_HOLDER 77))",
    );
    let result = run_one(scope, parse_one("answer"));
    assert!(
        matches!(result, KObject::Number(n) if *n == 77.0),
        "the closure arg invoked inside CALL_IT must read base=77, got {:?}",
        result.ktype(),
    );
}

/// A `let`-bound list reaching two *distinct* per-call regions keeps both alive: each `MAKE_HOLDER`
/// call captures its own per-call frame, and the list holds both closures. The bind's carrier fold
/// (C1) must contribute *every* region the multi-region value reaches — the case the single-frame
/// relocate-seam fold under-recorded. Reading the list back after the producing frames retire must
/// find both closures intact. (Miri: the multi-region no-use-after-free check.)
#[test]
fn let_bound_list_reaching_two_call_regions_keeps_both_live() {
    use crate::machine::model::{Held, KObject};
    let region = FrameStorage::run_root();
    let scope = run_root_silent(&region);
    run(
        scope,
        "FN (MAKE_HOLDER base :Number) -> :(FN (q :Number) -> Number) = \
         (FN (GET q :Number) -> Number = (base))\n\
         LET holders = [(MAKE_HOLDER 1) (MAKE_HOLDER 2)]",
    );
    let result = run_one(scope, parse_one("holders"));
    match result {
        KObject::List(items, _) => {
            assert_eq!(items.len(), 2, "list should hold both holder closures");
            assert!(
                items
                    .iter()
                    .all(|h| matches!(h, Held::Object(KObject::KFunction(_)))),
                "both list elements must be intact closures after their call regions retired",
            );
        }
        other => panic!("expected a List, got {:?}", other.ktype()),
    }
}
