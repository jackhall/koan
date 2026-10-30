//! The build arena's bounds: what no frame holds stays within the budget however a loop binds, and
//! what frames hold grows with the distinct arena nodes they run under, never with how deeply they
//! nest.

use std::num::NonZeroUsize;

use proptest::prelude::*;

use crate::program::CellSubstrate;
use crate::scheduler::{TIERS, Tiers};
use crate::values::BuildBudget;

use super::super::Koan;
use super::{WRITTEN, output};

/// `Holder`, `KEEP`, a callee keyed on the module it views, `Hop`, a chain link beside a module
/// and a quote, and three quotes written apart.
const PRELUDE: &str = "SIG Holder FOR ALL #[Elt] = #[(VAL held :Elt)]\n\
                       EXPR #(KEEP m :Holder) -> Any = #((LET w = (m :| Holder)) (w.held))\n\
                       NEWTYPE Hop = :{next :Any, m :Any, q :Any}\n\
                       LET q0 = #(1 + 1)\nLET q1 = #(2 + 2)\nLET q2 = #(3 + 3)\n";

/// [`PRELUDE`], `count` modules `m0`… each holding its index, a chain `h` of `depth` hops whose hop
/// `i` names module `pick(i)` and a quote by it, ending in `null`, and `source`.
fn program(count: usize, depth: usize, pick: impl Fn(usize) -> usize, source: &str) -> String {
    let mut program = String::from(PRELUDE);
    for at in 0..count {
        program.push_str(&format!("MODULE m{at} = ((LET held = {at}))\n"));
    }
    program.push_str("LET h0 = null\n");
    for at in 1..=depth {
        let picked = pick(at);
        program.push_str(&format!(
            "LET h{at} = (Hop {{next = h{}, m = m{}, q = q{}}})\n",
            at - 1,
            picked % count,
            picked % 3
        ));
    }
    program.push_str(&format!("LET h = h{depth}\n{source}"));
    program
}

/// Run `source` under `budget`: what it printed, the drain's tallies, and how many bytes no run
/// cell held at its end.
fn ran(source: &str, budget: BuildBudget) -> (String, Tiers, usize) {
    WRITTEN.with(|written| written.borrow_mut().clear());
    let mut substrate = CellSubstrate::load::<Koan>(source, "<test>", 4, budget, output())
        .unwrap_or_else(|error| panic!("the program loads: {error}"));
    TIERS.with(|tiers| tiers.set(Tiers::default()));
    substrate
        .with(|running| running.run())
        .expect("the program runs");
    let unheld = substrate.with(|running| running.scheduler().graph().unheld_bytes());
    let printed = WRITTEN.with(|written| written.borrow().join("\n"));
    (printed, TIERS.with(std::cell::Cell::get), unheld)
}

/// A loop down `h` that calls `KEEP` on each hop's module — a fresh load-content binding wherever
/// the module differs — and, where `composes`, composes and runs the hop's quote, so the arena holds
/// settlements and compositions both.
fn looping(composes: bool) -> String {
    let compose = match composes {
        true => "(LET qs = [(ATTR h q)]) (EVAL #($..qs) -> Any) ",
        false => "",
    };
    format!(
        "EXPR #(LOOP h :Hop) -> Number = \
         #((LET x = (KEEP (ATTR h m))) {compose}(LOOP (ATTR h next)))\n\
         EXPR #(LOOP end :Null) -> Number = #(7)\n\
         PRINT (LOOP h)"
    )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(if cfg!(miri) { 1 } else { 16 }))]

    #[test]
    fn build_memory_no_frame_holds_stays_within_the_budget(
        count in 1usize..6,
        depth in 1usize..24,
        seed in any::<u64>(),
        settlements in 1usize..4,
        composes in any::<bool>(),
    ) {
        // One settlement's weight sizes the budget, so a loop over more bindings must evict.
        let (_, _, one) = ran(
            &program(1, 1, |_| 0, "LET x = (KEEP m0)\n"),
            BuildBudget::default(),
        );
        let budget = BuildBudget {
            bytes: NonZeroUsize::new(settlements * one).expect("a settlement weighs"),
        };
        let pick = |at: usize| (seed.rotate_left(at as u32) as usize).wrapping_add(at);
        let (printed, tiers, _) = ran(&program(count, depth, pick, &looping(composes)), budget);
        prop_assert_eq!(printed, "7");
        // After every miss's eviction, what no run cell holds lies within the budget.
        prop_assert!(tiers.unheld <= budget.bytes.get(), "{:?} against {}", tiers, budget.bytes);
    }
}

/// `DOWN`, keyed on the module it views, which recurses down `h` and binds its value only once the
/// frame below returns, so every frame of the recursion is live at its bottom.
const DOWN: &str = "EXPR #(DOWN m :Holder h :Hop) -> Number = \
                    #((LET w = (m :| Holder)) (LET r = (DOWN (ATTR h m) (ATTR h next))) (r))\n\
                    EXPR #(DOWN m :Holder end :Null) -> Number = #(7)\n\
                    PRINT (DOWN m0 h)";

#[test]
fn what_frames_hold_is_linear_in_distinct_arena_nodes_not_nesting_depth() {
    // Every frame of a recursion under one binding runs under its one settlement, and two bindings
    // alternating hold two, however deep.
    for (count, live) in [(1, 1), (2, 2)] {
        let held = [4, 32].map(|depth| {
            let (printed, tiers, _) = ran(
                &program(count, depth, |at| at, DOWN),
                BuildBudget::default(),
            );
            assert_eq!(printed, "7");
            tiers.live
        });
        assert_eq!(held, [live, live], "{count} bindings");
    }
}
