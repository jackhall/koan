//! Region-liveness acceptance: a completed run releases **every** region it minted. The counter is
//! `region_metrics().live`, which drops only when a `RegionHost` actually drops, so a run that ends
//! with a live count above zero has an `Rc` cycle in its retention graph — the leak Miri reports as
//! a whole run retained at process exit, caught here in milliseconds instead.
//!
//! The shapes below are the ones that distinguish the retention rules from each other: a module
//! crossing into a per-call region as an argument makes that region retain the module's home (the
//! run root), and the call's result makes the run root retain the per-call owner — a two-region ring
//! that only the eternal rule (`PinBundle::without_eternal`) cuts.

use crate::builtins::test_support::TestRun;
use crate::machine::model::KObject;
use crate::machine::{program_storage, run_root_storage};
use crate::witnessed::{region_metrics, reset_region_metrics};

/// Run `source` to completion in a fresh run, drop the whole run, and report how many of its
/// regions are still live.
fn live_after(source: &str) -> usize {
    reset_region_metrics();
    let program = program_storage();
    // Mint the program storage region into the baseline rather than the run's toll: it holds the
    // source's AST for as long as anything reads it, exactly as production holds it for the run,
    // so it is alive on both sides of the measurement and cancels out. Resetting first and
    // baselining after keeps the counter from going negative when the storage finally drops.
    program.brand();
    let baseline = region_metrics().live;
    {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run(source);
    }
    region_metrics().live - baseline
}

/// A module value crossing into a per-call region as a call argument. The per-call region retains
/// the module's home region (the run root) for its own life; without the eternal rule the run root
/// retains the per-call owner right back, and neither ever frees.
#[test]
fn module_argument_leaves_no_live_region() {
    assert_eq!(
        live_after(
            "MODULE int_ord = (LET compare = 7)\n\
             FN (TAKESET elem :Module) -> Number = (1)\n\
             LET taken = (TAKESET int_ord)\n"
        ),
        0
    );
}

/// The functor-generativity shape from the Miri audit slate — a module argument crossing into two
/// successive per-call regions, each of which also returns a module of its own.
#[test]
fn functor_application_leaves_no_live_region() {
    assert_eq!(
        live_after(
            "SIG Ordered = (VAL compare :Number)\n\
             MODULE int_ord = (LET compare = 7)\n\
             LET int_ord_a = (int_ord :! Ordered)\n\
             FN (MAKESET elem :Ordered) -> Module = (MODULE generated = (LET inner = 1))\n\
             LET set_one = (MAKESET (int_ord_a))\n\
             LET set_two = (MAKESET (int_ord_a))\n"
        ),
        0
    );
}

/// The surrounding shapes, each exercising a different retention route: a bare module bind (no
/// per-call region at all), a record argument (copied, so its producer releases), a region-pure
/// call, a module returned from a call, an opaque view, a `USING` overlay fold, and a tail loop.
#[test]
fn every_call_shape_leaves_no_live_region() {
    let shapes: &[(&str, &str)] = &[
        ("module bound only", "MODULE int_ord = (LET compare = 7)\n"),
        (
            "record argument",
            "LET rec = ({a = 1})\n\
             FN (TAKEREC elem :Any) -> Number = (1)\n\
             LET taken = (TAKEREC rec)\n",
        ),
        (
            "region-pure call",
            "FN (NOOP) -> Number = (1)\n\
             LET taken = (NOOP)\n",
        ),
        (
            "module returned from a call",
            "FN (MAKETREE elem :Type) -> Module = (MODULE generated = (LET inner = 1))\n\
             LET made = (MAKETREE Number)\n",
        ),
        (
            "module argument, module return",
            "MODULE int_ord = (LET compare = 7)\n\
             FN (TAKESET elem :Module) -> Module = (MODULE out = (LET inner = 1))\n\
             LET taken = (TAKESET int_ord)\n",
        ),
        (
            "opaque ascription view",
            "SIG Ordered = ((TYPE Carrier) (VAL compare :Number))\n\
             MODULE int_ord = ((LET Carrier = Number) (LET compare = 7))\n\
             LET view = (int_ord :| Ordered)\n",
        ),
        (
            "USING overlay",
            "MODULE int_ord = (LET compare = 7)\n\
             LET seen = (USING int_ord (compare))\n",
        ),
        (
            "tail loop",
            "UNION Nat = (Zero :Null Succ :Nat)\n\
             FN (COUNTDOWN n :Nat) -> Str = (MATCH (n) OVER Nat -> :Str WITH (\
                 Zero -> (\"done\")\
                 Succ -> (COUNTDOWN it)\
             ))\n\
             LET n0 = (Nat.Zero null)\n\
             LET n1 = (Nat.Succ n0)\n\
             LET n2 = (Nat.Succ n1)\n\
             LET out = (COUNTDOWN n2)\n",
        ),
    ];
    for (name, source) in shapes {
        assert_eq!(live_after(source), 0, "shape `{name}` left a region live");
    }
}

/// Run `source` to completion in a fresh run and report the **peak** number of regions live at any
/// moment during it, alongside the count still live after the whole run drops.
fn peak_and_live_after(source: &str) -> (usize, usize) {
    reset_region_metrics();
    let program = program_storage();
    // Baselined exactly as [`live_after`] does, and subtracted from the peak too: the storage is
    // live for the whole window, so it inflates both readings by the same one region.
    program.brand();
    let baseline = region_metrics().live;
    let peak = {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&program, &region);
        test_run.run(source);
        region_metrics().peak
    };
    (peak - baseline, region_metrics().live - baseline)
}

/// A tail loop whose every iteration splices an eagerly-dispatched sub-result into its own working
/// expression: `(COUNTDOWN (PASS it))` evaluates `(PASS it)` as an eager dep and rests the resolved
/// cell in the dispatching step's own region.
fn splicing_countdown(depth: usize) -> String {
    let mut source = String::from(
        "UNION Nat = (Zero :Null Succ :Nat)\n\
         FN (PASS x :Nat) -> Nat = (x)\n\
         FN (COUNTDOWN n :Nat) -> Str = (MATCH (n) OVER Nat -> :Str WITH (\
             Zero -> (\"done\")\
             Succ -> (COUNTDOWN (PASS it))\
         ))\n\
         LET n0 = (Nat.Zero null)\n",
    );
    for i in 1..=depth {
        source.push_str(&format!("LET n{i} = (Nat.Succ n{})\n", i - 1));
    }
    source.push_str(&format!("LET out = (COUNTDOWN n{depth})\n"));
    source
}

/// **A spliced cell is adopted before the hop that retires it.** One hop is the whole
/// no-use-after-free shape: the sub-result rests as a pin-less `Sealed` cell in the dispatching
/// step's own region, and the incarnation that will run the body has a *freshly minted cart whose
/// ancestor chain does not reach that region* — so if the adoption happened on the far side of the
/// hop, nothing would be holding the cell up. It happens on the near side instead: the decide that
/// folds the call (`enter_user_fn`) lifts the cell and binds it into the new cart while the resting
/// region is still its own step's. Reading the result back is the check; tree borrows catches a
/// use-after-free if that ordering ever breaks.
#[test]
fn a_spliced_cell_is_adopted_before_its_tail_hop() {
    let program = program_storage();
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&program, &region);
    test_run.run(&splicing_countdown(1));
    let result = test_run.run_one(test_run.parse_one("out"));
    assert!(
        matches!(result, KObject::KString(s) if *s == "done"),
        "the spliced tail hop's result reads back intact, got {:?}",
        result.ktype()
    );
}

/// **Splice retention is per-iteration, not cumulative.** A spliced cell's pins live in the region
/// the splice site rested them into, released when that region dies — so a tail loop's producer
/// regions turn over with their consuming iteration instead of chaining forward. The observable is
/// the *peak* live-region count: it is flat in the loop's depth, where a retention that accumulated
/// would grow one region per hop.
///
/// The depths bracket the per-hop cost either way: the `LET n<i>` prelude is straight-line
/// construction, so any growth between them is the loop's own. It is a loud metric assert with no
/// Miri-only failure mode, so it runs under plain `cargo test`; the hop's use-after-free shape is
/// the one-hop slate twin above.
#[test]
fn a_splicing_tail_loop_holds_no_region_per_iteration() {
    let (shallow_peak, shallow_live) = peak_and_live_after(&splicing_countdown(3));
    let (deep_peak, deep_live) = peak_and_live_after(&splicing_countdown(11));
    assert_eq!(shallow_live, 0, "the shallow run leaves no region live");
    assert_eq!(deep_live, 0, "the deep run leaves no region live");
    assert_eq!(
        shallow_peak, deep_peak,
        "eight more tail hops must cost no extra live region — a spliced producer is released \
         with the iteration that consumed it"
    );
}
