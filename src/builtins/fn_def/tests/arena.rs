//! Run-root region and scheduler-slot reclamation invariants for user FN calls.

use crate::builtins::test_support::{parse_one, TestRun};
use crate::machine::core::KoanRegionExt;
use crate::machine::{program_storage, run_root_storage};
use crate::witnessed::region_metrics;

#[test]
fn chained_user_fn_tail_calls_reuse_one_slot() {
    let program = program_storage();
    let region = run_root_storage();
    let (mut test_run, captured) = TestRun::with_buf(&program, &region);
    let scope = test_run.scope;

    test_run.run(
        "FN (BB) -> Null = (PRINT \"ok\")\n\
         FN (AA) -> Null = (BB)",
    );

    // The slot count below is absolute, so release the definition statements' slots first: the
    // store's length is a high-water mark over the whole scheduler's life.
    test_run.reset_slots();
    let runtime = &mut test_run.runtime;
    runtime.dispatch_in_scope(
        crate::machine::model::WorkingExpression::from_ast(
            scope.brand(),
            parse_one(&program, "AA"),
        ),
        scope,
    );
    runtime.execute().expect("AA should run");

    assert_eq!(captured.borrow().as_slice(), b"ok\n");
    assert_eq!(
        runtime.len(),
        1,
        "tail-call slot reuse = AA -> BB -> PRINT should collapse into one slot, got {}",
        runtime.len(),
    );
}

#[test]
fn chained_tail_calls_reuse_frames() {
    let program = program_storage();
    let region = run_root_storage();
    let (mut test_run, captured) = TestRun::with_buf(&program, &region);
    let scope = test_run.scope;

    test_run.run(
        "FN (DD) -> Null = (PRINT \"ok\")\n\
         FN (CC) -> Null = (DD)\n\
         FN (BB) -> Null = (CC)\n\
         FN (AA) -> Null = (BB)",
    );

    // The slot count below is absolute, so release the definition statements' slots first: the
    // store's length is a high-water mark over the whole scheduler's life.
    test_run.reset_slots();
    let runtime = &mut test_run.runtime;
    // Parse before the snapshot: the parse bumps into program storage, and that storage's own
    // region mint is not a call's mint.
    let call = parse_one(&program, "AA");
    let minted_before = region_metrics().minted_total;
    runtime.dispatch_in_scope(
        crate::machine::model::WorkingExpression::from_ast(scope.brand(), call),
        scope,
    );
    runtime.execute().expect("AA should run");

    assert_eq!(captured.borrow().as_slice(), b"ok\n");
    assert_eq!(runtime.len(), 1, "tail chain should collapse to one slot");
    // A `FreshTail` mints a fresh region at every hop (the cart is never reused in place), so the
    // chain mints exactly one region per user-fn call: AA, BB, CC, DD. `minted_total` is
    // monotonic (never decremented by a drop), so a before/after diff is safe to read without
    // resetting the counters — a reset here would zero the live count out from under the
    // already-minted, still-alive run-root region and underflow at its eventual drop.
    let minted = region_metrics().minted_total - minted_before;
    assert_eq!(
        minted, 4,
        "expected exactly one region mint per user-fn call across AA -> BB -> CC -> DD, got {minted}",
    );
}

/// Side-effect ordering across a tail chain whose bodies each open with a value-discarded
/// leading `PRINT`. The leading statements are owned deps the slot parks on, so they run — and
/// finish — strictly before the tail continues: `a, b, c, d` (the leading PRINTs, in call order)
/// then `ok` (DD's body terminal). A fire-and-forget leading would race the tail chain and emit
/// the terminal first (`ok, a, b, c, d`).
#[test]
fn leading_statements_run_before_tail_across_chain() {
    let program = program_storage();
    let region = run_root_storage();
    let (mut test_run, captured) = TestRun::with_buf(&program, &region);

    test_run.run(
        "FN (DD) -> Str = ((PRINT \"d\") (PRINT \"ok\"))\n\
         FN (CC) -> Str = ((PRINT \"c\") (DD))\n\
         FN (BB) -> Str = ((PRINT \"b\") (CC))\n\
         FN (AA) -> Str = ((PRINT \"a\") (BB))",
    );

    test_run.run("AA");

    assert_eq!(
        String::from_utf8_lossy(&captured.borrow()),
        "a\nb\nc\nd\nok\n",
        "leading PRINTs must run in call order, each before its tail call continues",
    );
}

/// Tail chain whose bodies each carry a value-discarded leading `PRINT` stays TCO-flat: the
/// leading statements are owned deps that cascade-free as each call resolves, so they never
/// accumulate their own slots. The chain peaks at two slots — the tail-replaced main slot plus a
/// single leading-PRINT slot recycled through the free-list across all four calls. Fire-and-forget
/// leading would instead leave one orphan PRINT slot per call aliasing its frame (`runtime.len()`
/// would climb to 5).
#[test]
fn chained_tail_calls_with_leading_stay_tco_flat() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;

    test_run.run(
        "FN (DD) -> Str = ((PRINT \"d\") (PRINT \"ok\"))\n\
         FN (CC) -> Str = ((PRINT \"c\") (DD))\n\
         FN (BB) -> Str = ((PRINT \"b\") (CC))\n\
         FN (AA) -> Str = ((PRINT \"a\") (BB))",
    );

    // The slot count below is absolute, so release the definition statements' slots first: the
    // store's length is a high-water mark over the whole scheduler's life.
    test_run.reset_slots();
    let runtime = &mut test_run.runtime;
    // Parse before the snapshot: the parse bumps into program storage, and that storage's own
    // region mint is not a call's mint.
    let call = parse_one(&program, "AA");
    let minted_before = region_metrics().minted_total;
    runtime.dispatch_in_scope(
        crate::machine::model::WorkingExpression::from_ast(scope.brand(), call),
        scope,
    );
    runtime.execute().expect("AA should run");

    assert_eq!(
        runtime.len(),
        2,
        "leading statements are owned and cascade-free, so each PRINT slot is recycled via the \
         free-list rather than orphaned — the chain peaks at the main slot plus one reused \
         leading slot (a leak would climb to 5), got {}",
        runtime.len(),
    );
    // Each of AA, BB, CC, DD is its own user-fn call, so a `FreshTail` mints one region per call
    // regardless of the leading statement fanned into that call's already-installed frame.
    let minted = region_metrics().minted_total - minted_before;
    assert_eq!(
        minted, 4,
        "expected exactly one region mint per user-fn call across AA -> BB -> CC -> DD, got {minted}",
    );
}

/// Recursive tail-call through a `MATCH` arm completes in constant space: the
/// slot is reinstalled each hop and the library turns over its region; see
/// [tail-call-optimization.md](../../../../design/tail-call-optimization.md).
#[test]
fn match_driven_tail_recursion_completes() {
    let program = program_storage();
    let region = run_root_storage();
    let (mut test_run, captured) = TestRun::with_buf(&program, &region);

    test_run.run(
        "UNION Bit = (One :Null Zero :Null)\n\
         FN (HOP b :Any) -> Any = (MATCH (b) -> :Str WITH (\
             One -> (HOP (Bit (Zero null)))\
             Zero -> (PRINT \"done\")\
         ))",
    );

    test_run.run("HOP (Bit (One null))");

    assert_eq!(captured.borrow().as_slice(), b"done\n");
}

/// A MATCH arm whose body opens with a value-discarded leading `PRINT` before a tail-recursive
/// call. The arm runs through the action harness (`branch_walk` mints a `FreshChild` frame and
/// emits an `Action::Tail` carrying the leading statement), so this pins that the harness routes
/// arm-body leading statements through the same owned-dep park: the leading `PRINT` runs before
/// the recursion continues, giving `hop` (the One arm) then `done` (the Zero arm) in order.
#[test]
fn match_arm_leading_statement_runs_before_tail_recursion() {
    let program = program_storage();
    let region = run_root_storage();
    let (mut test_run, captured) = TestRun::with_buf(&program, &region);

    test_run.run(
        "UNION Bit = (One :Null Zero :Null)\n\
         FN (HOP b :Any) -> Any = (MATCH (b) -> :Str WITH (\
             One -> ((PRINT \"hop\") (HOP (Bit (Zero null))))\
             Zero -> (PRINT \"done\")\
         ))",
    );

    test_run.run("HOP (Bit (One null))");

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
    use crate::machine::KErrorKind;
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    let scope = test_run.scope;
    test_run.run(
        "FN (GG) -> Str = (\"hello\")\n\
         FN (FF) -> Number = (GG)",
    );
    let runtime = &mut test_run.runtime;
    let id = runtime.dispatch_in_scope(
        crate::machine::model::WorkingExpression::from_ast(
            scope.brand(),
            parse_one(&program, "FF"),
        ),
        scope,
    );
    runtime
        .execute()
        .expect("execute does not surface per-slot errors");
    let err = match runtime.result_error(id) {
        Err(e) => e,
        Ok(()) => panic!("FF -> Number tail-calling GG -> Str must fail FF's return contract"),
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
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(
        "FN (GG) -> :(LIST OF Number) = ([1 2 3])\n\
         FN (FF) -> :(LIST OF Any) = (GG)",
    );
    let result = test_run.run_one(parse_one(&program, "FF"));
    match result {
        KObject::List(_, list_type) => assert_eq!(
            *list_type,
            test_run.types.list(KType::ANY),
            "FF -> (LIST OF Any) must coarsen the tail-chain result to List<Any>, got {list_type:?}",
        ),
        other => panic!("expected a List from FF, got {:?}", other.ktype()),
    }
}

/// A MATCH arm's `-> :T` contract, checked across a tail chain **longer than two hops** through
/// plain user-defined functions (`AA -> BB -> CC`) — long enough that every transient cart from the
/// arm's own era has long since been reused/dropped by the time the chain resolves. The MATCH is
/// dispatched directly (not nested inside an enclosing FN call, whose own declared-return contract
/// would win keep-first and mask the arm's), so the arm's own contract is genuinely the chain's
/// first. The arm's home region can only still be checked against because the contract's own
/// carried witness (its home scope's region owner, folded into a `CarrierWitness` singleton at seal
/// time) pins it directly, independent of any surviving cart's `outer` chain.
#[test]
fn deep_tail_chain_satisfies_arm_return_contract() {
    use crate::machine::model::KObject;
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(
        "UNION Bit = (One :Null Zero :Null)\n\
         FN (CC) -> Any = (\"ok\")\n\
         FN (BB) -> Any = (CC)\n\
         FN (AA) -> Any = (BB)\n\
         LET b = (Bit (One null))",
    );
    let types = test_run.types.clone();
    let result = test_run.run_one(parse_one(
        &program,
        "MATCH (b) -> :Str WITH (\
                 One -> (AA)\
                 Zero -> (\"unused\")\
             )",
    ));
    assert!(
        matches!(result, KObject::KString(s) if *s == "ok"),
        "expected the MATCH arm's :Str contract to pass the 3-hop tail chain's Str result, got {}",
        result.ktype().name(&types),
    );
}

/// The violating twin of [`deep_tail_chain_satisfies_arm_return_contract`]: the same 3-hop chain
/// resolves to a `Number`, which the MATCH arm's `:Str` contract must still reject — proving the
/// check itself (not just a passing value) survives the chain, since a dangling home region would
/// either panic/UB under Miri or silently skip the check rather than raise this error.
#[test]
fn deep_tail_chain_violates_arm_return_contract() {
    use crate::machine::KErrorKind;
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(
        "UNION Bit = (One :Null Zero :Null)\n\
         FN (CC) -> Any = (42)\n\
         FN (BB) -> Any = (CC)\n\
         FN (AA) -> Any = (BB)\n\
         LET b = (Bit (One null))",
    );
    let err = test_run.run_one_err(parse_one(
        &program,
        "MATCH (b) -> :Str WITH (\
                 One -> (AA)\
                 Zero -> (\"unused\")\
             )",
    ));
    assert!(
        matches!(err.kind, KErrorKind::TypeMismatch { ref arg, .. } if arg == "<return>"),
        "expected a <return> TypeMismatch against the MATCH arm's :Str contract, got {err}",
    );
}

#[test]
fn repeated_user_fn_calls_do_not_grow_run_root_per_call() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run("FN (ECHO v :Number) -> Number = (v)");
    // `allocated_total` weighs both halves of the region: the two typed sub-arenas and the bump
    // the `Drop`-free families live in. A per-call leak into run-root shows up in one or the other,
    // so the growth bound has to read them together.
    //
    // The bump half reports reserved chunk capacity, which arrives in chunk-sized steps that double
    // as the bump grows. Two things keep that from reading as a leak. A warmup window climbs the
    // chunk ladder before the baseline is taken. And the measured window is long enough that
    // per-call growth dominates a single step: below a few hundred calls the whole window fits
    // inside one chunk, so the reading is that chunk's size and says nothing about per-call cost —
    // it is flat from 100 calls to 200, then linear.
    const WARMUP: usize = 50;
    const CALLS: u64 = 400;
    for _ in 0..WARMUP {
        let _ = test_run.run_one(parse_one(&program, "ECHO 7"));
    }
    let baseline = region.region().allocated_total();
    for _ in 0..CALLS {
        let _ = test_run.run_one(parse_one(&program, "ECHO 7"));
    }
    let growth = region.region().allocated_total() - baseline;
    // Measured at six `KObject`-sized cells per call over the linear stretch; the bound leaves 3x
    // slack, which still catches any regression that re-introduces a per-call leak into run-root.
    // Six rather than the four cells a live-byte tally would report: chunks double, so reserved
    // capacity runs to roughly twice what is live in it.
    let budget = CALLS * 18 * std::mem::size_of::<crate::machine::model::KObject<'_>>() as u64;
    assert!(
        growth < budget,
        "per-call leak regression: {growth} new run-root bytes across {CALLS} ECHO calls \
         (expected < {budget})",
    );
}

/// Property: after a warmup call populates the free-list with the body's
/// transient slots, steady-state per-call growth in `nodes.len()` is bounded
/// by the *persistent* per-call overhead — the top-level dispatch slot plus
/// any persistent shim. Without reclamation, every call would leave its
/// body's transient fanout (~5+ slots/call) behind.
#[test]
fn body_subexpression_slots_recycle_across_calls() {
    let program = program_storage();
    let region = run_root_storage();
    let (mut test_run, captured) = TestRun::with_buf(&program, &region);
    let scope = test_run.scope;

    test_run.run(
        "UNION Bit = (One :Null Zero :Null)\n\
         FN (LOOK b :Any) -> Any = (MATCH (b) -> :Str WITH (\
             One -> (PRINT \"one\")\
             Zero -> (PRINT \"zero\")\
         ))",
    );

    let runtime = &mut test_run.runtime;

    // Warmup: populates the free-list with the body's transient pool.
    runtime.dispatch_in_scope(
        crate::machine::model::WorkingExpression::from_ast(
            scope.brand(),
            parse_one(&program, "LOOK (Bit (One null))"),
        ),
        scope,
    );
    runtime.execute().expect("LOOK should run");
    let after_warmup = runtime.len();

    let n = 30;
    for i in 1..=n {
        let src = if i % 2 == 0 {
            "LOOK (Bit (One null))"
        } else {
            "LOOK (Bit (Zero null))"
        };
        runtime.dispatch_in_scope(
            crate::machine::model::WorkingExpression::from_ast(
                scope.brand(),
                parse_one(&program, src),
            ),
            scope,
        );
        runtime.execute().expect("LOOK should run");
    }
    let after_batch = runtime.len();

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
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(
        "FN (MAKE_HOLDER base :Number) -> :(FN (q :Number) -> Number) = \
         (FN (GET q :Number) -> Number = (base))\n\
         LET hold = (MAKE_HOLDER 99)",
    );
    let result = test_run.run_one(parse_one(&program, "hold {q = 0}"));
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
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(
        "FN (MAKE_HOLDER base :Number) -> :(FN (q :Number) -> Number) = \
         (FN (GET q :Number) -> Number = (base))\n\
         FN (CALL_IT f :(FN (q :Number) -> Number)) -> Number = (f {q = 0})\n\
         LET answer = (CALL_IT (MAKE_HOLDER 77))",
    );
    let result = test_run.run_one(parse_one(&program, "answer"));
    assert!(
        matches!(result, KObject::Number(n) if *n == 77.0),
        "the closure arg invoked inside CALL_IT must read base=77, got {:?}",
        result.ktype(),
    );
}

/// One `let`-bound list mixing the two cell shapes whose reach verdicts disagree, over four
/// *distinct* per-call regions, read back after every producer retires.
///
/// A **closure** cell rides a bare borrow into its defining region and never gets rebuilt, so the
/// bind's carrier fold must contribute *every* region the multi-region value reaches — the case the
/// single-frame relocate-seam fold under-recorded. A **string** cell answers the opposite way: its
/// reach verdict names no region at all, honest only because the substrate door re-bumps the bytes
/// into the list's own region first (`section_cells`); a cell that skipped the re-home would name no
/// region while still pointing into a retiring one. Interleaving them puts one fold under both rules
/// at once — drop the closure's region and the captured read dangles, share the producer's string
/// bump and the byte read dangles. (Miri: the multi-region and region-resident-string
/// no-use-after-free checks in one scheduler run.)
#[test]
fn let_bound_list_of_call_produced_strings_and_closures_survives_every_producer_free() {
    use crate::machine::model::{Held, KObject};
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(
        "FN (MAKE_HOLDER base :Number) -> :(FN (q :Number) -> Number) = \
         (FN (GET q :Number) -> Number = (base))\n\
         FN (LABEL n :Number) -> Str = (PRINT n)\n\
         LET mixed = [(LABEL 1) (MAKE_HOLDER 1) (LABEL 2) (MAKE_HOLDER 2)]",
    );
    let result = test_run.run_one(parse_one(&program, "mixed"));
    match result {
        KObject::List(items, _) => {
            let cells = items.elements();
            assert_eq!(
                cells.len(),
                4,
                "list should hold both labels and both holders"
            );
            let rendered: Vec<&str> = cells
                .iter()
                .step_by(2)
                .map(|cell| match cell {
                    Held::Object(KObject::KString(s)) => *s,
                    other => panic!(
                        "expected a string cell, got {:?}",
                        other.ktype(&test_run.types)
                    ),
                })
                .collect();
            assert_eq!(
                rendered,
                vec!["1", "2"],
                "both string cells must read back intact after their call regions retired",
            );
            assert!(
                cells
                    .iter()
                    .skip(1)
                    .step_by(2)
                    .all(|h| matches!(h, Held::Object(KObject::KFunction(_)))),
                "both closure cells must stay intact after their distinct call regions retired",
            );
        }
        other => panic!("expected a List, got {:?}", other.ktype()),
    }
}

/// The same check one layer in: a **dict** whose *keys* are call-produced strings. Keys ride a
/// separate path from value cells — they are staged out of their producer's envelope and re-bumped
/// as the key→index table freezes (`alloc_dict`) — so a key that kept its producer's pointer would
/// dangle where a value cell would not. Looking the entry up after the producers retire both reads
/// the stored key bytes (the `str` compare) and proves they are still there.
#[test]
fn let_bound_dict_with_call_produced_string_keys_survives_every_producer_free() {
    use crate::machine::model::{Held, KKey, KObject};
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(
        "FN (LABEL n :Number) -> Str = (PRINT n)\n\
         LET entries = {(LABEL 1): 10, (LABEL 2): 20}",
    );
    let result = test_run.run_one(parse_one(&program, "entries"));
    match result {
        KObject::Dict(substrate, _) => {
            for (key, expected) in [("1", 10.0), ("2", 20.0)] {
                match substrate.entry(&KKey::String(key)) {
                    Some(Held::Object(KObject::Number(n))) => assert_eq!(*n, expected),
                    _ => panic!("expected `{key}` bound to {expected}"),
                }
            }
        }
        other => panic!("expected a Dict, got {:?}", other.ktype()),
    }
}
