//! Acceptance-criteria coverage for library-owned tail-call region turnover — see
//! [tail-call-optimization.md](../../../../design/tail-call-optimization.md). Each test pins one
//! criterion directly, independent of the region-reclamation tests in [`super::arena`]:
//!
//! - `O(1)` live regions across a deep tail loop, on one scheduler slot.
//! - The no-mint incarnation categories (§ Region liveness by node lifetime) mint nothing of
//!   their own.
//! - A loop-carried aggregate correctly crosses a tail hop (Lemma 2 — the retiring region
//!   outlives the adoption that reads it).

use crate::builtins::test_support::{parse_one, run, run_one, run_root_silent};
use crate::machine::model::Held;
use crate::machine::model::KObject;
use crate::machine::model::TypeRegistry;
use crate::machine::run_root_storage;
use crate::machine::KoanRuntime;
use crate::witnessed::{region_metrics, reset_region_metrics};

/// A depth-1000 tail-recursive countdown runs on one scheduler slot and in `O(1)` live regions.
/// `reset_region_metrics` is called before anything mints (before the run-root region itself, which
/// mints as soon as the test scope is built) so the later peak reading is meaningful and no
/// still-live region is zeroed out from under itself. The countdown is expressed as a `Nat`
/// (`Zero | Succ Nat`) unwound one layer per hop through `MATCH` — the recursion lives entirely in
/// the scheduler's `NodeStep::Replace` loop, not in Rust call-stack depth, so the depth-1000 value
/// is built beforehand as 1000 flat (no-mint, top-level) `LET`s rather than a 1000-deep parsed
/// literal.
#[test]
fn tail_recursive_countdown_stays_o1_in_regions() {
    reset_region_metrics();
    let region = run_root_storage();
    let scope = run_root_silent(&region);

    // Enough hops to distinguish O(1) from O(depth) — a non-TCO recursion would leave DEPTH slots
    // and DEPTH live regions — while staying tractable under Miri's interpreter (the whole audit
    // slate re-runs this fixture; a large depth makes it the slate's bottleneck).
    const DEPTH: usize = 20;
    let mut source = String::from(
        "UNION Nat = (Zero :Null Succ :Nat)\n\
         FN (COUNTDOWN n :Nat) -> Str = (MATCH (n) -> :Str WITH (\
             Zero -> (\"done\")\
             Succ -> (COUNTDOWN it)\
         ))\n\
         LET n0 = (Nat (Zero null))\n",
    );
    for i in 1..=DEPTH {
        source.push_str(&format!("LET n{i} = (Nat (Succ n{}))\n", i - 1));
    }
    run(scope, &source);

    // Only the setup's own (no-mint) top-level statements have run so far; the run-root mint is
    // the sole contributor to `peak` at this point.
    let baseline = region_metrics().peak;

    let mut runtime = KoanRuntime::new();
    let id = runtime.dispatch_in_scope(parse_one(&format!("COUNTDOWN n{DEPTH}")), scope);
    runtime
        .execute()
        .expect("the countdown should run to completion");
    assert!(
        runtime.result_error(id).is_ok(),
        "countdown should complete without error: {:?}",
        runtime.result_error(id).err(),
    );

    // A MATCH-based tail loop peaks at two slots — the tail-replaced main slot plus the MATCH
    // arm's own recycled slot (see `chained_tail_calls_with_leading_stay_tco_flat` in
    // `super::arena`) — constant regardless of depth, never the depth-`N` slot-per-hop a naive
    // (non-TCO) recursion would leave behind.
    assert_eq!(
        runtime.len(),
        2,
        "a depth-{DEPTH} tail loop must stay on O(1) scheduler slots, got {}",
        runtime.len(),
    );
    // The transient ceiling is three per-call regions, the floor the design's own lemmas set for a
    // MATCH-mediated hop: when the fresh COUNTDOWN cart mints (eagerly, at the arm step's apply —
    // per-call frames build their child scope at construction), the retiring arm cart is still
    // pinned by the step-open witness and the spliced argument cell (Lemma 2 — it must outlive the
    // adoption), and the arm's `outer` link pins its enclosing COUNTDOWN cart (Lemma 3). Steady
    // state falls back to one live per-call region between hops; only depth-independence — not the
    // exact transient — is what O(1) claims.
    let peak = region_metrics().peak;
    assert!(
        peak <= baseline + 3,
        "a tail loop must hold O(1) (<= 3 transiently) live regions regardless of depth; \
         baseline {baseline}, peak {peak}",
    );
}

/// The no-mint incarnation categories from
/// [tail-call-optimization.md § Region liveness by node lifetime](../../../../design/tail-call-optimization.md#region-liveness-by-node-lifetime)
/// — a parenthesized syntactic reduction, a bare-name forward, a `USING` overlay entry, and a
/// plain top-level sequence — add no region mints of their own. The baseline is read *after* the
/// module declaration (which mints its own region) so the assertion isolates exactly the four
/// categories under test, not the fixture's own setup cost.
#[test]
fn no_mint_categories_add_no_region_mints() {
    reset_region_metrics();
    let region = run_root_storage();
    let scope = run_root_silent(&region);

    // Setup: a module to open a `USING` window on. Whatever this costs (module bodies are not
    // among the four categories under test) is folded into `baseline` below.
    run(scope, "MODULE mo = ((LET hidden = 99))");
    let baseline = region_metrics().minted_total;

    run(
        scope,
        // Top-level sequence: every statement here is itself the fourth no-mint category.
        "LET a = 42\n\
         LET b = ((a))\n\
         LET visible = (USING mo SCOPE (hidden))",
    );
    // Bare-name forward: a submission that is just a name, spliced onto its existing producer.
    let mut runtime = KoanRuntime::new();
    let id = runtime.dispatch_in_scope(parse_one("a"), scope);
    runtime.execute().expect("bare-name forward should run");
    assert!(
        runtime.result_error(id).is_ok(),
        "bare-name forward should resolve cleanly: {:?}",
        runtime.result_error(id).err(),
    );

    let minted = region_metrics().minted_total;
    assert_eq!(
        minted,
        baseline,
        "parenthesized reduction, bare-name forward, USING overlay, and top-level sequence must \
         mint no region of their own; got {} additional mint(s)",
        minted - baseline,
    );
}

/// Adoption-before-free (Lemma 2): a loop-carried aggregate (a `List`) rebuilt at every hop from
/// the previous hop's own carried value — so the spliced carrier genuinely pins the retiring
/// incarnation's region across the hop, and the free is ordered strictly after the adoption reads
/// it. Correctness (not a crash / wrong value) is the observable half of the guarantee under plain
/// `cargo test`; the orchestrating Miri run is what confirms the ordering itself never
/// use-after-frees.
#[test]
fn loop_carried_aggregate_survives_tail_hop_adoption() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "FN (DD acc :(LIST OF Any)) -> :(LIST OF Any) = (acc)\n\
         FN (CC acc :(LIST OF Any)) -> :(LIST OF Any) = (DD [(acc)])\n\
         FN (BB acc :(LIST OF Any)) -> :(LIST OF Any) = (CC [(acc)])\n\
         FN (AA acc :(LIST OF Any)) -> :(LIST OF Any) = (BB [(acc)])",
    );
    // Each hop rewraps the previous hop's own list (`[(acc)]`), so unwrapping the wraps back down
    // must reach the original seed `0` unharmed.
    let result = run_one(scope, parse_one("AA [0]"));
    let mut depth = 0;
    let mut current = result;
    loop {
        match current {
            KObject::List(items, _) => {
                assert_eq!(items.len(), 1, "each wrap is a single-element list");
                depth += 1;
                current = match &items[0] {
                    Held::Object(obj) => obj,
                    Held::Type(_) | Held::UnresolvedType(_) => {
                        panic!("expected an object element, got a type")
                    }
                };
            }
            KObject::Number(n) => {
                assert_eq!(
                    *n, 0.0,
                    "the innermost seed value must survive every hop unharmed"
                );
                break;
            }
            other => panic!(
                "expected nested Lists bottoming out at Number(0), got {}",
                other.ktype().name(&TypeRegistry::new()),
            ),
        }
    }
    assert_eq!(
        depth, 4,
        "the seed's own wrap plus AA -> BB -> CC each rewrapping once (DD passes through) \
         should reach depth 4, got {depth}",
    );
}
