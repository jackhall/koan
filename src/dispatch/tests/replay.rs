//! The stamp and replay: a callable keeps the requests that built the chain of settlements it was
//! born under, and a call of it whose arena node was evicted replays them, so its frame is born
//! warm.

use std::cell::Cell;

use crate::program::CellSubstrate;
use crate::scheduler::{TIERS, Tiers};
use crate::values::BuildBudget;

use super::super::Koan;
use super::caching::{PRELUDE, SMALLEST};
use super::{WRITTEN, output};

/// Two modules a `Holder` reads, and `MAKE`, keyed on one, whose frame births a function keyed
/// on another.
const MAKE: &str = "MODULE one = (LET held = 1)\n\
                    MODULE two = (LET held = 2)\n\
                    EXPR #(MAKE m :Holder) -> Any = #((LET w = (m :| Holder)) \
                    (FN :{k :Holder} -> Number = #((LET v = (k :| Holder)) (7))))\n";

/// What one run of `source` under `budget` printed, beside the drain's tallies.
fn replayed(source: &str, budget: BuildBudget) -> (String, Tiers) {
    WRITTEN.with(|written| written.borrow_mut().clear());
    let mut substrate = CellSubstrate::load::<Koan>(source, "<test>", 4, budget, output())
        .unwrap_or_else(|error| panic!("the program loads: {error}"));
    TIERS.with(|tiers| tiers.set(Tiers::default()));
    substrate
        .with(|running| running.run())
        .expect("the program runs");
    let printed = WRITTEN.with(|written| written.borrow().join("\n"));
    (printed, TIERS.with(Cell::get))
}

#[test]
fn a_call_through_a_stale_handle_rebuilds_its_chain_and_is_born_warm() {
    let calls = "LET g = (MAKE one)\nPRINT (g {k = one})\nLET h = (MAKE two)\nPRINT (g {k = one})";
    let (printed, tiers) = replayed(&format!("{PRELUDE}{MAKE}{calls}"), SMALLEST);
    let (roomy, _) = replayed(&format!("{PRELUDE}{MAKE}{calls}"), BuildBudget::default());
    assert_eq!(printed, "7\n7");
    assert_eq!(printed, roomy);
    // `MAKE two`'s miss evicts `g`'s chain; the second call of `g` rebuilds `MAKE one`'s settlement
    // and settles its own under it again, born warm.
    assert_eq!(tiers.cold_births, 0);
    assert_eq!(tiers.settled, 5);
}

#[test]
fn a_replay_borrows_every_level_it_reaches_until_the_frame_is_born() {
    let nested = "EXPR #(ONE a :Holder) -> Any = #((LET w = (a :| Holder)) \
                  (FN :{b :Holder} -> Any = #((LET x = (b :| Holder)) \
                  (FN :{c :Holder} -> Any = #((LET y = (c :| Holder)) \
                  (FN :{n :Number} -> Number = #(n + 1)))))))\n\
                  LET second = (ONE one)\nLET third = (second {b = one})\n\
                  LET fourth = (third {c = one})\nLET other = (MAKE two)\n\
                  PRINT (fourth {n = 1})";
    let (printed, tiers) = replayed(&format!("{PRELUDE}{MAKE}{nested}"), SMALLEST);
    assert_eq!(printed, "2");
    // Three levels settled, evicted by `MAKE two`'s miss, and settled again by the replay.
    assert_eq!(tiers.cold_births, 0);
    assert_eq!(tiers.settled, 7);
}

#[test]
fn a_replay_rebuilds_the_composition_its_chain_pins() {
    let (printed, tiers) = replayed(
        "LET q = #((SIG Hold FOR ALL #[Elt] = #[(VAL held :Elt)]) \
         (LET make = (FN :{m :Hold} -> Any = #((LET v = (m :| Hold)) \
         (FN :{n :Number} -> Number = #(n + 1))))) (MODULE m = ((LET held = 1))) (make {m = m}))
LET g = (EVAL (COMPOSE q) -> Any)
LET p = #(PRINT 0)
EVAL (COMPOSE p) -> Any
PRINT (g {n = 1})",
        SMALLEST,
    );
    assert_eq!(printed, "0\n2");
    // `p`'s miss evicts `q`'s composition and the settlement under it: the call bounces to rebuild
    // the composition, then replays the settlement under it.
    assert_eq!(
        (tiers.composed, tiers.settled, tiers.cold_births),
        (3, 2, 0)
    );
}

#[test]
fn a_non_keying_callable_borrows_the_settlement_its_handle_names() {
    let source = "EXPR #(KEEP m :Holder) -> Any = #((LET w = (m :| Holder)) \
                  (FN :{n :Number} -> Any = #((LET inner = (MAKE two)) (n))))\n\
                  LET g = (KEEP one)\nPRINT (g {n = 1})\nPRINT (g {n = 2})";
    let (printed, tiers) = replayed(&format!("{PRELUDE}{MAKE}{source}"), SMALLEST);
    assert_eq!(printed, "1\n2");
    // `g`'s frame borrows `KEEP one`'s settlement, so the miss inside it does not evict it: the
    // second call finds it live and replays nothing, and hits `MAKE two`'s.
    assert_eq!((tiers.settled, tiers.cold_births), (2, 0));
}

#[test]
fn the_stamp_is_no_part_of_a_closures_content() {
    let (printed, _) = replayed(
        &format!("{PRELUDE}{MAKE}LET g = (MAKE one)\nLET h = (MAKE two)\nPRINT (g == h)"),
        BuildBudget::default(),
    );
    // Born under two settlements, so two stamps, over one code and no capture.
    assert_eq!(printed, "true");
}

#[test]
fn a_closure_returned_from_its_frame_replays_after_the_frames_region_is_gone() {
    let source = "EXPR #(WRAP m :Holder) -> Any = #((LET made = (MAKE m)) (made))\n\
                  LET g = (WRAP one)\nLET h = (MAKE two)\nPRINT (g {k = one})";
    let (printed, tiers) = replayed(&format!("{PRELUDE}{MAKE}{source}"), SMALLEST);
    assert_eq!(printed, "7");
    assert_eq!(tiers.cold_births, 0);
}
