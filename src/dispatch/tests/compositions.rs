//! Code composed at run time, through the test-only `COMPOSE`: an `EVAL` of it runs under its
//! composition, found by its syntax digest or built, which rests nothing in the agent tier and
//! reports what its code refuses as the load words it.

use std::cell::Cell;

use crate::program::CellSubstrate;
use crate::scheduler::{TIERS, Tiers};
use crate::type_lattice::ADOPT_LAID;
use crate::values::BuildBudget;

use super::super::Koan;
use super::caching::{PRELUDE, SMALLEST, program};
use super::{WRITTEN, output, run};

/// What one run of `source` left: what it printed, the drain's tallies, and how many compositions
/// were live at its end.
fn composed(source: &str) -> (String, Tiers, usize) {
    WRITTEN.with(|written| written.borrow_mut().clear());
    let mut substrate =
        CellSubstrate::load::<Koan>(source, "<test>", 4, BuildBudget::default(), output())
            .unwrap_or_else(|error| panic!("the program loads: {error}"));
    TIERS.with(|tiers| tiers.set(Tiers::default()));
    substrate
        .with(|running| running.run())
        .expect("the program runs");
    let live = substrate.with(|running| running.scheduler().graph().live_compositions());
    let printed = WRITTEN.with(|written| written.borrow().join("\n"));
    (printed, TIERS.with(Cell::get), live)
}

#[test]
fn an_eval_of_composed_code_runs_as_its_written_quote_does() {
    let (printed, tiers, live) = composed(
        "LET x = 1
LET q = #((LET y = 2) (PRINT (y + $x)))
EVAL q -> Any
EVAL (COMPOSE q) -> Any",
    );
    assert_eq!(printed, "3\n3");
    assert_eq!((tiers.composed, live), (1, 1));
}

#[test]
fn a_second_eval_of_equal_composed_code_hits_its_composition() {
    let (printed, tiers, live) = composed(
        "LET x = 1
LET q = #(PRINT (2 + $x))
LET p = #(PRINT (2 + $x))
EVAL (COMPOSE q) -> Any
EVAL (COMPOSE q) -> Any
EVAL (COMPOSE p) -> Any",
    );
    assert_eq!(printed, "3\n3\n3");
    // Two quotes written apart are one syntax, so one composition.
    assert_eq!((tiers.composed, live), (1, 1));
}

#[test]
fn a_loop_composing_code_each_hop_holds_o1_compositions() {
    let looping = |depth| {
        let (printed, tiers, live) = composed(&program(
            depth,
            "LET q = #(1 + 1)
EXPR #(LOOP link :Link) -> Number = #((EVAL (COMPOSE q) -> Any) (LOOP (ATTR link next)))
EXPR #(LOOP end :Null) -> Number = #(7)
PRINT (LOOP chain)",
        ));
        assert_eq!(printed, "7");
        (tiers.composed, live)
    };
    assert_eq!(looping(4), (1, 1));
    assert_eq!(looping(16), (1, 1));
}

#[test]
fn composed_code_that_does_not_type_refuses_at_its_eval_worded_as_the_load() {
    for code in [
        "#((LET x = 1) (LET x = 2))",
        "#((LET y = \"a\") (PRINT (y + 1)))",
        "#(PRINT \\w)",
    ] {
        let written = run(&format!("LET q = {code}\nEVAL q -> Any"));
        let composed = run(&format!("LET q = {code}\nEVAL (COMPOSE q) -> Any"));
        assert!(written.starts_with("error: "), "`{code}`: {written}");
        assert_eq!(composed, written, "`{code}`");
    }
}

#[test]
fn a_composition_rests_nothing_in_the_agent_tier() {
    let (printed, tiers, _) = composed(
        "LET x = 1
LET Alias = Number
LET q = #((LET r = {a = $x}) (NEWTYPE Boxed = :(LIST OF $Alias)) \
         (LET f = (FN :{n :($Alias)} -> Number = #(n + 1))) (PRINT [r (Boxed [1]) (f {n = 2})]))
EVAL (COMPOSE q) -> Any",
    );
    assert_eq!(printed, "[{a = 1}, Boxed([1]), 3]");
    assert_eq!(tiers.agent, 0);
}

#[test]
fn a_composed_invariant_site_adopts_nothing_into_the_agent_tier() {
    let (printed, tiers, _) = composed(
        "LET q = #((SIG Hold FOR ALL #[Elt] = #[(VAL held :Elt)]) (MODULE m = ((LET held = 1))) \
         (LET peek = (FN :{m :Hold} -> Str = #((LET v = (m :| Hold)) (\"x\")))) \
         (PRINT (peek {m = m})) (PRINT (peek {m = m})))
EVAL (COMPOSE q) -> Any",
    );
    assert_eq!(printed, "x\nx");
    assert_eq!(tiers.agent, 0);
}

/// `OUTER`, a written operator keyed on a module, whose invariant site reads it as a `Holder`.
const OUTER: &str = "EXPR #(OUTER m :Holder) -> Number = #((LET w = (m :| Holder)) (7))\n";

#[test]
fn a_composed_asker_calling_a_written_keyed_operator_settles_it() {
    let (printed, tiers, _) = composed(&format!(
        "{PRELUDE}{OUTER}LET q = #((MODULE m = ((LET held = 1))) (PRINT $(OUTER m)))
EVAL (COMPOSE q) -> Any"
    ));
    assert_eq!(printed, "7");
    // The module is the one the load computed for the written quote: load content, so the call
    // settles under the root.
    assert_eq!((tiers.sought, tiers.settled), (1, 1));
}

#[test]
fn a_written_callee_under_the_root_bound_to_a_composed_module_relays_nothing() {
    let (printed, tiers, _) = composed(&format!(
        "{PRELUDE}{OUTER}LET f = (FN :{{n :Number}} -> Number = #(n))
LET x = (f {{n = 1}})
LET q = #((MODULE m = ((LET held = $x))) (PRINT $(OUTER m)))
EVAL (COMPOSE q) -> Any"
    ));
    assert_eq!(printed, "7");
    // No layer of the callee's chain holds the module's tree: the call seeks nothing.
    assert_eq!((tiers.sought, tiers.relayed, tiers.agent), (0, 0, 0));
}

#[test]
fn a_derived_type_under_a_composition_is_minted_once_per_binding_event() {
    let minted = |evals: usize| {
        let source = format!(
            "LET x = 1\nLET q = #((LET r = {{a = $x}}) (PRINT r))\n{}",
            "EVAL (COMPOSE q) -> Any\n".repeat(evals)
        );
        let laid = ADOPT_LAID.with(Cell::get);
        let (_, tiers, _) = composed(&source);
        assert_eq!(tiers.composed, 1);
        ADOPT_LAID.with(Cell::get) - laid
    };
    let (once, twice) = (minted(1), minted(2));
    assert!(once > 0, "the record over `$x` is a term the frame mints");
    // Each `EVAL` is its own binding event, and mints the composition's terms once.
    assert_eq!(twice, 2 * once);
}

/// What one run of `source` under the smallest budget printed, with how many compositions it built.
fn smallest(source: &str) -> (String, Tiers) {
    WRITTEN.with(|written| written.borrow_mut().clear());
    let mut substrate = CellSubstrate::load::<Koan>(source, "<test>", 4, SMALLEST, output())
        .unwrap_or_else(|error| panic!("the program loads: {error}"));
    TIERS.with(|tiers| tiers.set(Tiers::default()));
    substrate
        .with(|running| running.run())
        .expect("the program runs");
    let printed = WRITTEN.with(|written| written.borrow().join("\n"));
    (printed, TIERS.with(Cell::get))
}

#[test]
fn a_composition_evicted_and_rebuilt_gives_its_functions_the_same_identity() {
    let (printed, tiers) = smallest(
        "LET q = #(FN :{n :Number} -> Number = #(n + 1))
LET p = #(PRINT 0)
LET f = (EVAL (COMPOSE q) -> Any)
EVAL (COMPOSE p) -> Any
LET g = (EVAL (COMPOSE q) -> Any)
PRINT (f == g)
EVAL (COMPOSE p) -> Any
PRINT (f {n = 1})",
    );
    // `p`'s miss evicts `q`'s composition, which `g`'s rebuilds; then `p`'s evicts it again, and
    // the call of `f` rebuilds it from the root `f` keeps. Each miss bounces once with its syntax.
    assert_eq!(printed, "0\ntrue\n0\n2");
    assert_eq!((tiers.composed, tiers.bounced), (5, 5));
}

#[test]
fn a_quote_born_of_composed_code_runs_under_its_composition() {
    let (printed, tiers) = smallest(
        "LET q = #((LET inner = #(PRINT 5)) (EVAL inner -> Any) (inner))
LET p = #(PRINT 0)
LET i = (EVAL (COMPOSE q) -> Any)
EVAL (COMPOSE p) -> Any
EVAL i -> Any
PRINT i",
    );
    // The quote keeps its syntax inside its root's, so it outlives its composition.
    assert_eq!(printed, "5\n0\n5\nPRINT 5");
    assert_eq!(tiers.composed, 3);
}

#[test]
fn a_using_over_a_quote_born_of_composed_code_fills_its_holes() {
    let source = |eval: &str| {
        format!(
            "LET q = #((LET inner = #(PRINT (x + 1))) (inner))
LET p = #(PRINT 0)
LET i = ({eval} -> Any)
EVAL (COMPOSE p) -> Any
LET j = (i USING {{x = 4}})
EVAL j -> Any"
        )
    };
    let (printed, tiers) = smallest(&source("EVAL (COMPOSE q)"));
    // `p`'s miss evicts `q`'s composition between the birth of `i` and the `USING`, which
    // rebuilds it to fill the hole there; the `EVAL` of `j` finds it.
    assert_eq!(printed, "0\n5");
    assert_eq!(printed, smallest(&source("EVAL q")).0);
    assert_eq!((tiers.composed, tiers.bounced), (3, 3));
}

#[test]
fn a_module_born_of_composed_code_reads_as_one_written() {
    let source = |eval: &str| {
        format!(
            "LET q = #((MODULE m = ((LET x = 1) (LET y = 2))) m)\nLET m = ({eval} -> Any)\nPRINT m.y"
        )
    };
    assert_eq!(run(&source("EVAL (COMPOSE q)")), run(&source("EVAL q")));
}

#[test]
fn a_call_of_a_function_born_of_composed_code_reads_its_compositions_types() {
    let (printed, tiers, _) = composed(
        "LET Alias = Number
LET q = #((LET g = (FN :{n :($Alias)} -> Number = #(n + 1))) (g))
LET f = (EVAL (COMPOSE q) -> Any)
PRINT (f {n = 1})
PRINT (f {n = 2})",
    );
    assert_eq!(printed, "2\n3");
    assert_eq!(tiers.agent, 0);
}

/// A nominal declared over a `$` type the composition cannot see is declared by the frame at its
/// declaring statement, as the written quote's is, and rests nothing in the agent tier.
#[test]
fn a_composed_declared_nominal_is_minted_at_its_declaring_statement() {
    let source = |eval: &str| {
        format!(
            "LET Alias = Number\nLET q = #((NEWTYPE Boxed = :(LIST OF $Alias)) (PRINT (Boxed [1, 2])))\n{eval} -> Any"
        )
    };
    let (composed, tiers, _) = composed(&source("EVAL (COMPOSE q)"));
    assert_eq!(composed, run(&source("EVAL q")));
    assert_eq!(composed, "Boxed([1, 2])");
    assert_eq!(tiers.agent, 0);
}
