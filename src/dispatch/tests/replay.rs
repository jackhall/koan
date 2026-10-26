//! The stamp and replay: a callable keeps the requests that built the chain of settlements it was
//! born under, and a call of it whose arena node was evicted replays them, so its frame is born
//! warm. A frame's level is laid once, and only where its callee's body can birth a callable: by
//! the frame in its own region, never in its asker's.

use std::cell::Cell;

use crate::dispatch::evaluate::ASKER_BYTES;
use crate::program::{CellSubstrate, LEVELS};
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

/// `PASS`, a `Str`-returning loop calling the `Number`-returning `TOUCH` each hop, which keys on
/// `m` where `keyed` and births a callable where `births`.
fn touching(keyed: bool, births: bool) -> String {
    let view = if keyed {
        "(LET v = (m :| Counter)) "
    } else {
        ""
    };
    let born = if births {
        "(LET f = (FN :{n :Number} -> Number = #(n))) "
    } else {
        ""
    };
    format!(
        "EXPR #(TOUCH m :Counter) -> Number = #({view}{born}(1))\n\
         EXPR #(PASS m :Counter link :Link) -> Str = \
         #((LET v = (m :| Counter)) (LET t = (TOUCH m)) (PASS m (ATTR link next)))\n\
         EXPR #(PASS m :Counter end :Null) -> Str = #(\"done\")\n\
         PRINT (PASS ints chain)"
    )
}

#[test]
fn a_keyed_call_each_hop_lays_no_level_in_its_asker() {
    for births in [false, true] {
        // The most bytes `PASS`'s region held at a call of `TOUCH`, keyed or not, at `depth` hops.
        let asker = |keyed: bool, depth: u32| {
            ASKER_BYTES.with(|most| most.set(0));
            let ran =
                super::caching::ran_under(depth, &touching(keyed, births), BuildBudget::default());
            assert_eq!(ran.printed, "done");
            // Keyed, `PASS` and `TOUCH` each settle once; the control keys `PASS` alone.
            assert_eq!(ran.tiers.settled, if keyed { 2 } else { 1 });
            ASKER_BYTES.with(Cell::get)
        };
        let (keyed, control) = (
            (asker(true, 40), asker(true, 80)),
            (asker(false, 40), asker(false, 80)),
        );
        // Each hop's `PASS` is a frame of its own, so what it holds at the call is the same however
        // deep the loop, and a keyed call lays nothing there the control does not.
        assert_eq!(
            keyed.0, keyed.1,
            "births {births}: keyed {keyed:?} at 40 and 80 hops"
        );
        assert_eq!(control.0, control.1, "births {births}: control {control:?}");
        assert!(
            keyed.1 <= control.1,
            "births {births}: keyed {keyed:?} lays no more than control {control:?}"
        );
    }
}

#[test]
fn a_frame_lays_its_level_once_and_only_where_its_body_can_birth_a_callable() {
    let calls = "EXPR #(TWICE m :Holder) -> Any = #((LET w = (m :| Holder)) \
                 (LET f = (FN :{n :Number} -> Number = #(n))) (FN :{n :Number} -> Number = #(n + 1)))\n\
                 EXPR #(COUNT m :Holder) -> Number = #((LET w = (m :| Holder)) \
                 (LET f = (FN :{n :Number} -> Number = #(n))) (f {n = 3}))\n\
                 EXPR #(PEEK m :Holder) -> Number = #((LET w = (m :| Holder)) (1))\n\
                 LET a = (TWICE one)\nLET b = (TWICE one)\nLET c = (COUNT one)\nLET d = (PEEK one)\n\
                 PRINT (b {n = 1})\nPRINT c\nPRINT d";
    LEVELS.with(|levels| levels.set(0));
    let (printed, _) = replayed(&format!("{PRELUDE}{MAKE}{calls}"), BuildBudget::default());
    assert_eq!(printed, "2\n3\n1");
    // One for each `TWICE` frame, though each births two callables, which its asker lays; one for
    // `COUNT`, which its own frame lays from the request handed back; none for `PEEK`.
    assert_eq!(LEVELS.with(Cell::get), 3);
}

#[test]
fn a_callable_born_in_a_block_replays_its_frames_level() {
    let source = "EXPR #(BLOCKY m :Holder) -> Any = #((LET w = (m :| Holder)) \
                  (USING empty SCOPE (FN :{k :Holder} -> Number = #((LET v = (k :| Holder)) (7)))))\n\
                  LET g = (BLOCKY one)\nPRINT (g {k = one})\nLET h = (MAKE two)\nPRINT (g {k = one})";
    let (printed, tiers) = replayed(&format!("{PRELUDE}{MAKE}{source}"), SMALLEST);
    assert_eq!(printed, "7\n7");
    // As through a stale handle born in the frame's own statements: `g`, born in a block, records
    // `BLOCKY one`'s level, so its second call rebuilds that settlement and is born warm.
    assert_eq!(tiers.cold_births, 0);
    assert_eq!(tiers.settled, 5);
}

#[test]
fn a_replay_reaches_a_fresh_frames_level() {
    let source = "EXPR #(OUTER m :Holder) -> Number = #((LET w = (m :| Holder)) \
                  (LET g = (FN :{k :Holder} -> Any = #((LET v = (k :| Holder)) \
                  (FN :{n :Number} -> Number = #(n + 1))))) \
                  (LET h = (g {k = one})) (LET other = (MAKE two)) (h {n = 1}))\n\
                  PRINT (OUTER one)";
    let (printed, tiers) = replayed(&format!("{PRELUDE}{MAKE}{source}"), SMALLEST);
    assert_eq!(printed, "2");
    // `OUTER`, `g` and `MAKE two` seek once each. `MAKE two`'s miss evicts `g`'s settlement, which
    // no frame holds, but not `OUTER`'s, which `OUTER`'s frame borrows. `h`'s call is then born cold
    // under the root and replays `h`'s stamp: `OUTER`'s level, laid by `OUTER`'s `Fresh` frame and
    // found live, then `g`'s, settled again. Without `OUTER`'s level the replay would seek `g`'s
    // alone, under the root: four seeks.
    assert_eq!((tiers.sought, tiers.settled, tiers.cold_births), (5, 4, 0));
}
