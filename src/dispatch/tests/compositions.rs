//! Code composed at run time, a splice of written quotes: an `EVAL` of it runs under its
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
LET q = #((LET y = 2) (PRINT (y + $x)))\nLET qs = [q]
EVAL q -> Any
EVAL #($..qs) -> Any",
    );
    assert_eq!(printed, "3\n3");
    assert_eq!((tiers.composed, live), (1, 1));
}

#[test]
fn a_second_eval_of_equal_composed_code_hits_its_composition() {
    let (printed, tiers, live) = composed(
        "LET x = 1
LET q = #(PRINT (2 + $x))\nLET qs = [q]
LET p = #(PRINT (2 + $x))\nLET ps = [p]
EVAL #($..qs) -> Any
EVAL #($..qs) -> Any
EVAL #($..ps) -> Any",
    );
    assert_eq!(printed, "3\n3\n3");
    // Two quotes written apart are one syntax, so one composition.
    assert_eq!((tiers.composed, live), (1, 1));
}

/// What one run of a loop composing code each hop left, under `budget`.
#[derive(Debug, PartialEq, Eq)]
struct Looped {
    printed: String,
    /// The most any step left in its region and its scratch.
    region: usize,
    scratch: usize,
    /// How many compositions the run built and evicted.
    composed: usize,
    evicted: usize,
    /// The unheld bytes of the arena at the run's end.
    unheld: usize,
    /// The nodes adoption laid and the bytes program storage grew by, over the run.
    laid: usize,
    storage: usize,
}

/// Run `source` under `budget`, metered as [`Looped`] says.
fn looped(source: &str, budget: BuildBudget) -> Looped {
    WRITTEN.with(|written| written.borrow_mut().clear());
    let mut substrate = CellSubstrate::load::<Koan>(source, "<test>", 4, budget, output())
        .unwrap_or_else(|error| panic!("the program loads: {error}"));
    TIERS.with(|tiers| tiers.set(Tiers::default()));
    let (laid, storage) = (ADOPT_LAID.with(Cell::get), substrate.storage_bytes());
    substrate
        .with(|running| running.run())
        .expect("the program runs");
    let tiers = TIERS.with(Cell::get);
    Looped {
        printed: WRITTEN.with(|written| written.borrow().join("\n")),
        region: tiers.region,
        scratch: tiers.scratch,
        composed: tiers.composed,
        evicted: tiers.evicted,
        unheld: substrate.with(|running| running.scheduler().graph().unheld_bytes()),
        laid: ADOPT_LAID.with(Cell::get) - laid,
        storage: substrate.storage_bytes() - storage,
    }
}

#[test]
fn a_loop_composing_code_each_hop_holds_o1_compositions() {
    // Three quotes composed each hop, under a budget of one and a half compositions: every `EVAL`
    // misses, evicting the least recently used.
    let quotes = "LET q = #(1 + 1)\nLET qs = [q]\nLET r = #(2 + 2)\nLET rs = [r]\nLET s = #(3 + 3)\nLET ss = [s]\n";
    let one = looped(
        &format!("{quotes}EVAL #($..qs) -> Any"),
        BuildBudget::default(),
    );
    let budget = BuildBudget {
        bytes: std::num::NonZeroUsize::new(3 * one.unheld / 2).expect("a composition weighs"),
    };
    let looping = |depth| {
        looped(
            &program(
                depth,
                &format!(
                    "{quotes}EXPR #(LOOP link :Link) -> Number = #((EVAL #($..qs) -> Any) \
                     (EVAL #($..rs) -> Any) (EVAL #($..ss) -> Any) (LOOP (ATTR link next)))
EXPR #(LOOP end :Null) -> Number = #(7)
PRINT (LOOP chain)"
                ),
            ),
            budget,
        )
    };
    let (few, many) = (looping(4), looping(16));
    for run in [&few, &many] {
        assert_eq!(run.printed, "7");
        // Eviction runs at a miss, while the new composition is held: once the run ends, that one
        // lies unheld beside what the budget kept.
        assert!(run.unheld <= budget.bytes.get() + one.unheld, "{run:?}");
        assert_eq!(
            run.storage, 0,
            "a composition lays nothing in program storage"
        );
    }
    // Every hop composes afresh: three misses a hop, each evicting.
    assert_eq!((few.composed, many.composed), (12, 48));
    assert!(many.evicted >= 45, "{many:?}");
    // Region and scratch stay flat with depth, and no hop lays a node by adoption.
    assert_eq!((few.region, few.scratch), (many.region, many.scratch));
    assert_eq!(few.laid, many.laid);
}

#[test]
fn composed_code_that_does_not_type_refuses_at_its_eval_worded_as_the_load() {
    for code in [
        "#((LET x = 1) (LET x = 2))",
        "#((LET y = \"a\") (PRINT (y + 1)))",
        "#(PRINT \\w)",
    ] {
        let written = run(&format!("LET q = {code}\nEVAL q -> Any"));
        let composed = run(&format!(
            "LET q = {code}\nLET qs = [q]\nEVAL #($..qs) -> Any"
        ));
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
         (LET f = (FN :{n :($Alias)} -> Number = #(n + 1))) (PRINT [r (Boxed [1]) (f {n = 2})]))\nLET qs = [q]
EVAL #($..qs) -> Any",
    );
    assert_eq!(printed, "[{a = 1}, Boxed([1]), 3]");
    assert_eq!(tiers.agent, 0);
}

#[test]
fn a_composed_invariant_site_rests_nothing_in_the_agent_tier() {
    let (printed, tiers, _) = composed(
        "LET q = #((SIG Hold FOR ALL #[Elt] = #[(VAL held :Elt)]) (MODULE m = ((LET held = 1))) \
         (LET peek = (FN :{m :Hold} -> Str = #((LET v = (m :| Hold)) (\"x\")))) \
         (PRINT (peek {m = m})) (PRINT (peek {m = m})))\nLET qs = [q]
EVAL #($..qs) -> Any",
    );
    assert_eq!(printed, "x\nx");
    assert_eq!(tiers.agent, 0);
}

/// `OUTER`, a written operator keyed on a module, whose invariant site reads it as a `Holder`.
const OUTER: &str = "EXPR #(OUTER m :Holder) -> Number = #((LET w = (m :| Holder)) (7))\n";

#[test]
fn a_composed_asker_calling_a_written_keyed_operator_settles_it() {
    let (printed, tiers, _) = composed(&format!(
        "{PRELUDE}{OUTER}LET q = #((MODULE m = ((LET held = 1))) (PRINT $(OUTER m)))\nLET qs = [q]
EVAL #($..qs) -> Any"
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
LET q = #((MODULE m = ((LET held = $x))) (PRINT $(OUTER m)))\nLET qs = [q]
EVAL #($..qs) -> Any"
    ));
    assert_eq!(printed, "7");
    // No layer of the callee's chain holds the module's tree: the call seeks nothing.
    assert_eq!((tiers.sought, tiers.relayed, tiers.agent), (0, 0, 0));
}

#[test]
fn a_derived_type_under_a_composition_is_minted_once_per_binding_event() {
    let minted = |evals: usize| {
        let source = format!(
            "LET x = 1\nLET q = #((LET r = {{a = $x}}) (PRINT r))\nLET qs = [q]\n{}",
            "EVAL #($..qs) -> Any\n".repeat(evals)
        );
        let laid = ADOPT_LAID.with(Cell::get);
        let (_, tiers, _) = composed(&source);
        assert_eq!(tiers.composed, 1);
        ADOPT_LAID.with(Cell::get) - laid
    };
    let (once, twice) = (minted(1), minted(2));
    assert!(once > 0, "the record over `$x` is a term the frame mints");
    // Each `EVAL` is its own binding event, whose activation walk mints the composition's terms once.
    assert_eq!(twice, 2 * once);
}

/// What the calls of `f`, a function composed code returns beside `unused` more functions whose
/// types are over the `$` name it splices too, left once `calls` of them ran: the types their
/// frames found their chains lacking, and the nodes the run laid.
fn lazily(unused: usize, calls: usize) -> (usize, usize) {
    let mut code = String::from("#(");
    for at in 0..unused {
        code.push_str(&format!(
            "(LET u{at} = (FN :{{s :Str}} -> Any = #((LET r :{{a{at} :($Alias)}} = {{a{at} = 1}}) (r)))) "
        ));
    }
    code.push_str("(FN :{n :Number} -> Any = #((LET r :{b :($Alias)} = {b = n}) (r))))");
    let source = format!(
        "LET Alias = Number\nLET q = {code}\nLET qs = [q]\nLET f = (EVAL #($..qs) -> Any)\n{}",
        "PRINT (f {n = 1})\n".repeat(calls)
    );
    crate::scope::TERM_PROBES.with(|probes| probes.set(0));
    let laid = ADOPT_LAID.with(Cell::get);
    let (printed, tiers, _) = composed(&source);
    assert_eq!(printed, vec!["{b = 1}"; calls].join("\n"));
    assert_eq!(tiers.composed, 1);
    (
        crate::scope::TERM_PROBES.with(Cell::get),
        ADOPT_LAID.with(Cell::get) - laid,
    )
}

#[test]
fn a_frame_under_a_composition_probes_no_term_it_does_not_read() {
    // A call's frame mints what its reads name and nothing else, so a composition naming eight
    // more types costs each call nothing more: the `EVAL`'s activation walk mints them once.
    let per_call = |unused: usize| {
        let (once, twice) = (lazily(unused, 1), lazily(unused, 2));
        (twice.0 - once.0, twice.1 - once.1)
    };
    assert!(
        lazily(8, 1).0 > lazily(0, 1).0,
        "the eight are terms the `EVAL`'s walk mints"
    );
    assert_eq!(per_call(8), per_call(0));
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
        "LET q = #(FN :{n :Number} -> Number = #(n + 1))\nLET qs = [q]
LET p = #(PRINT 0)\nLET ps = [p]
LET f = (EVAL #($..qs) -> Any)
EVAL #($..ps) -> Any
LET g = (EVAL #($..qs) -> Any)
PRINT (f == g)
EVAL #($..ps) -> Any
PRINT (f {n = 1})",
    );
    // `p`'s miss evicts `q`'s composition, which `g`'s rebuilds; then `p`'s evicts it again, and
    // the call of `f` rebuilds it from the root `f` keeps. Each miss bounces once with its syntax.
    assert_eq!(printed, "0\ntrue\n0\n2");
    assert_eq!((tiers.composed, tiers.bounced), (5, 5));
}

#[test]
fn build_after_an_eviction_rebuilds_the_composition() {
    let (printed, tiers) = smallest(
        "LET a = [#(PRINT 1)]
LET b = [#(PRINT 0)]
LET built = (BUILD #($..a))
EVAL #($..b) -> Any
LET again = (BUILD #($..a))
PRINT (built == again)
EVAL again -> Any",
    );
    // `b`'s miss evicts the composition `a`'s first `BUILD` built, which the second rebuilds and
    // the `EVAL` finds.
    assert_eq!(printed, "0\ntrue\n1");
    assert_eq!(tiers.composed, 3);
}

#[test]
fn a_quote_born_of_composed_code_runs_under_its_composition() {
    let (printed, tiers) = smallest(
        "LET q = #((LET inner = #(PRINT 5)) (EVAL inner -> Any) (inner))\nLET qs = [q]
LET p = #(PRINT 0)\nLET ps = [p]
LET i = (EVAL #($..qs) -> Any)
EVAL #($..ps) -> Any
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
            "LET q = #((LET inner = #(PRINT (x + 1))) (inner))\nLET qs = [q]
LET p = #(PRINT 0)\nLET ps = [p]
LET i = ({eval} -> Any)
EVAL #($..ps) -> Any
LET j = (i USING {{x = 4}})
EVAL j -> Any"
        )
    };
    let (printed, tiers) = smallest(&source("EVAL #($..qs)"));
    // `p`'s miss evicts `q`'s composition between the birth of `i` and the `USING`, which
    // rebuilds it to fill the hole there; `j`'s syntax holds the filled mark, so its `EVAL` builds
    // a composition of its own.
    assert_eq!(printed, "0\n5");
    assert_eq!(printed, smallest(&source("EVAL q")).0);
    assert_eq!((tiers.composed, tiers.bounced), (4, 4));
}

#[test]
fn a_module_born_of_composed_code_reads_as_one_written() {
    let source = |eval: &str| {
        format!(
            "LET q = #((MODULE m = ((LET x = 1) (LET y = 2))) (m))\nLET qs = [q]\nLET m = ({eval} -> Any)\nPRINT m.y"
        )
    };
    assert_eq!(run(&source("EVAL q")), "2");
    assert_eq!(run(&source("EVAL #($..qs)")), "2");
}

#[test]
fn a_call_of_a_function_born_of_composed_code_reads_its_compositions_types() {
    let (printed, tiers, _) = composed(
        "LET Alias = Number
LET q = #((LET g = (FN :{n :($Alias)} -> Number = #(n + 1))) (g))\nLET qs = [q]
LET f = (EVAL #($..qs) -> Any)
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
            "LET Alias = Number\nLET q = #((NEWTYPE Boxed = :(LIST OF $Alias)) (PRINT (Boxed [1, 2])))\nLET qs = [q]\n{eval} -> Any"
        )
    };
    let (composed, tiers, _) = composed(&source("EVAL #($..qs)"));
    assert_eq!(composed, run(&source("EVAL q")));
    assert_eq!(composed, "Boxed([1, 2])");
    assert_eq!(tiers.agent, 0);
}

/// What one run of `source` left under `budget`: what it printed, the drain's tallies, and how
/// many settlements lay under a composition and how many compositions some settlement pinned at
/// its end.
fn pinned(source: &str, budget: BuildBudget) -> (String, Tiers, (usize, usize)) {
    WRITTEN.with(|written| written.borrow_mut().clear());
    let mut substrate = CellSubstrate::load::<Koan>(source, "<test>", 4, budget, output())
        .unwrap_or_else(|error| panic!("the program loads: {error}"));
    TIERS.with(|tiers| tiers.set(Tiers::default()));
    substrate
        .with(|running| running.run())
        .expect("the program runs");
    let under = substrate.with(|running| running.scheduler().graph().under_compositions());
    let printed = WRITTEN.with(|written| written.borrow().join("\n"));
    (printed, TIERS.with(Cell::get), under)
}

/// `PEEK`'s code: a `SIG` and a function keyed on a module it reads as one.
const PEEK: &str = "(SIG Hold FOR ALL #[Elt] = #[(VAL held :Elt)]) \
                    (LET peek = (FN :{m :Hold} -> Any = #((LET v = (m :| Hold)) ({h = v.held}))))";

/// A module whose `MODULE` is composed where it is born, its body spliced in: no shape the load
/// builds holds it, so only its composition computes its tree.
const FRESH: &str = "LET body = [#(((LET held = 1)))]\n\
                     LET m = (EVAL #((MODULE m = $..body) (m)) -> Any)\n";

#[test]
fn a_callable_born_in_composed_code_keys_a_settlement_under_its_composition() {
    let (printed, tiers, (under, pins)) = pinned(
        &format!(
            "LET q = #({PEEK} (MODULE m = ((LET held = 1))) (PRINT (peek {{m = m}})) \
             (PRINT (peek {{m = m}})))\nLET qs = [q]\nEVAL #($..qs) -> Any"
        ),
        BuildBudget::default(),
    );
    assert_eq!(printed, "{h = Elt(1)}\n{h = Elt(1)}");
    // The second call hits the settlement the first built, under the composition.
    assert_eq!((tiers.sought, tiers.settled), (2, 1));
    assert_eq!((under, pins), (1, 0));
}

#[test]
fn a_settlement_under_a_composition_rests_nothing_in_the_agent_tier() {
    let source = |eval: &str| {
        format!(
            "LET q = #({PEEK} (MODULE m = ((LET held = 1))) (PRINT (peek {{m = m}})))\nLET qs = [q]\n\
             {eval} -> Any"
        )
    };
    let (printed, tiers, _) = pinned(&source("EVAL #($..qs)"), BuildBudget::default());
    assert_eq!(printed, run(&source("EVAL q")));
    assert_eq!(tiers.settled, 1);
    assert_eq!((tiers.agent, tiers.relayed, tiers.rested), (0, 0, 0));
}

#[test]
fn a_settlement_binding_a_module_another_composition_computed_pins_it() {
    let (printed, tiers, (under, pins)) = pinned(
        &format!(
            "{FRESH}LET q = #({PEEK} (peek))\nLET qs = [q]\nLET peek = (EVAL #($..qs) -> Any)\n\
             PRINT (peek {{m = m}})\nPRINT (peek {{m = m}})"
        ),
        BuildBudget::default(),
    );
    assert_eq!(printed, "{h = Elt(1)}\n{h = Elt(1)}");
    assert_eq!((tiers.sought, tiers.settled, tiers.agent), (2, 1, 0));
    // The settlement lies under `peek`'s composition and pins `m`'s.
    assert_eq!((under, pins), (1, 1));
}

#[test]
fn a_pinned_composition_is_evicted_only_after_its_last_pinner() {
    let (printed, tiers, (under, pins)) = pinned(
        &format!(
            "{FRESH}LET q = #({PEEK} (peek))\nLET qs = [q]\nLET peek = (EVAL #($..qs) -> Any)\n\
             PRINT (peek {{m = m}})\nPRINT (peek {{m = m}})\nLET p = #(PRINT 0)\nLET ps = [p]\n\
             EVAL #($..ps) -> Any"
        ),
        SMALLEST,
    );
    assert_eq!(printed, "{h = Elt(1)}\n{h = Elt(1)}\n0");
    // `peek`'s miss evicts `m`'s composition, so the call rebuilds it from the module's root to pin
    // it — a fourth composition, and like each, built over the syntax a bounce copied; while the
    // settlement lives it stays, and the second call hits.
    assert_eq!((tiers.settled, tiers.composed, tiers.bounced), (1, 4, 4));
    // `p`'s miss evicts the settlement, then the composition it pinned, then `peek`'s.
    assert_eq!(tiers.evicted, 4);
    assert_eq!((under, pins), (0, 0));
}

#[test]
fn a_written_callee_bound_to_a_composed_module_decides_in_its_activation() {
    let (printed, tiers, _) = pinned(
        &format!("{PRELUDE}{OUTER}{FRESH}PRINT (OUTER m)"),
        BuildBudget::default(),
    );
    assert_eq!(printed, "7");
    // Under the root a settlement binds load content alone: `m`'s tree lies in its composition.
    assert_eq!((tiers.sought, tiers.settled, tiers.agent), (0, 0, 0));
}
