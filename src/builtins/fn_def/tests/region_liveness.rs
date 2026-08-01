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
use crate::machine::run_root_storage;
use crate::witnessed::{region_metrics, reset_region_metrics};

/// Run `source` to completion in a fresh run, drop the whole run, and report how many of its
/// regions are still live.
fn live_after(source: &str) -> usize {
    reset_region_metrics();
    {
        let region = run_root_storage();
        let mut test_run = TestRun::silent(&region);
        test_run.run(source);
    }
    region_metrics().live
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
             FN (COUNTDOWN n :Nat) -> Str = (MATCH (n) -> :Str WITH (\
                 Zero -> (\"done\")\
                 Succ -> (COUNTDOWN it)\
             ))\n\
             LET n0 = (Nat (Zero null))\n\
             LET n1 = (Nat (Succ n0))\n\
             LET n2 = (Nat (Succ n1))\n\
             LET out = (COUNTDOWN n2)\n",
        ),
    ];
    for (name, source) in shapes {
        assert_eq!(live_after(source), 0, "shape `{name}` left a region live");
    }
}
