//! The load's layers: a traced call, whose own site fixes its callee's binding, finds the layer
//! the load laid for it in the load's table, and settles nothing at run; a refusal one of those
//! layers reaches refuses the load, naming the call; two bindings keeping a body's questions share
//! one witnessed layer, which a settlement of another binding reads over, as does one of a callee
//! a capped body passes a slot on to. A call at a site a layer carries a request
//! for reads it and computes no key; any other computes its key from the values. A miss lays its
//! one layer, and a child at the child's first call; run content at a traced slot seeks nothing,
//! its frame deciding in its activation. Each test reads the printed output and the drain's
//! tallies, and some which seeks read a request; a key the load computes otherwise than the run
//! shows as `laid 0, settled 1`.

use std::cell::{Cell, RefCell};

use crate::program::TRACE_CAP as CAP;
use crate::program::{CellSubstrate, DECIDING, KBirth, LEVELS, REQUESTING, SEEKING};
use crate::scheduler::{ANSWERED, Sought, TIERS, Tiers};
use crate::scope::{EventSite, ShapeKind, Slot};
use crate::values::{BuildBudget, DepotRef, LayerKey};

use super::super::Koan;
use super::caching::{Ran, SMALLEST, program, ran_under};
use super::keys::{TWIN_WRAP, TWINS, declared_refusal};
use super::{WRITTEN, output};

/// Two overloads of `PICK`, one at `Number` and one at `Str`, and `BOTH`, whose `PICK a` waits on
/// `Elt`: a `FOR ALL` name its key lists.
const BOTH: &str = "EXPR #(PICK x :Number) -> Str = #(\"n\")\n\
                    EXPR #(PICK x :Str) -> Number = #(1)\n\
                    EXPR FOR ALL #[Elt] #(BOTH a :Elt AND b :Elt) -> Any = #((LET p = (PICK a)) (p))\n";

/// `CARRIER`, keyed on the module it views, and `one`, a module a `Holder` reads.
const CARRIER: &str = "EXPR #(CARRIER m :Counter) -> Number = #((LET v = (m :| Counter)) (7))\n\
                       MODULE one = (LET held = 1)\n";

/// Calls of `CARRIER` over views of `words` and then `ints` composed and evaluated at run:
/// untraced, so each settles at run, and under the smallest budget each miss evicts every
/// settlement no frame holds.
const EVICTING: &str = "LET a = (CARRIER (EVAL #($words :| $Counter) -> Counter))\n\
                        LET b = (CARRIER (EVAL #($ints :| $Counter) -> Counter))\n";

/// Run `source` over [`CARRIER`] once under `budget`.
fn ran(source: &str, budget: BuildBudget) -> Ran {
    ran_under(0, &format!("{CARRIER}{source}"), budget)
}

#[test]
fn a_keyworded_call_fixing_a_for_all_name_finds_the_loads_layer() {
    let ran = ran(
        &format!("{BOTH}PRINT (BOTH 1 AND 2)"),
        BuildBudget::default(),
    );
    assert_eq!(ran.printed, "n");
    assert_eq!(
        (ran.tiers.sought, ran.tiers.laid, ran.tiers.settled),
        (1, 1, 0)
    );
}

#[test]
fn a_functor_applied_to_a_written_module_finds_the_loads_layer() {
    let ran = ran("PRINT (CARRIER ints)", BuildBudget::default());
    assert_eq!(ran.printed, "7");
    assert_eq!(
        (ran.tiers.sought, ran.tiers.laid, ran.tiers.settled),
        (1, 1, 0)
    );
}

#[test]
fn a_call_over_a_view_finds_the_loads_layer() {
    let ran = ran("PRINT (CARRIER (ints :| Counter))", BuildBudget::default());
    assert_eq!(ran.printed, "7");
    assert_eq!(
        (ran.tiers.sought, ran.tiers.laid, ran.tiers.settled),
        (1, 1, 0)
    );
}

#[test]
fn a_call_over_a_view_the_trace_cannot_see_settles_at_run() {
    let ran = ran(
        "PRINT (CARRIER (EVAL #($ints :| $Counter) -> Counter))",
        BuildBudget::default(),
    );
    assert_eq!(ran.printed, "7");
    assert_eq!(
        (ran.tiers.sought, ran.tiers.laid, ran.tiers.settled),
        (1, 0, 1)
    );
}

#[test]
fn a_retyped_module_finds_the_loads_layer() {
    let ran = ran("PRINT (CARRIER (ints :! Counter))", BuildBudget::default());
    assert_eq!(ran.printed, "7");
    assert_eq!(
        (ran.tiers.sought, ran.tiers.laid, ran.tiers.settled),
        (1, 1, 0)
    );
}

#[test]
fn a_call_by_name_finds_the_loads_layer() {
    let ran = ran(
        "MODULE lib = (\n  LET pick = (FN FOR ALL #[Elt] :{x :Elt} -> Elt = #(x))\n  \
         PRINT (pick {x = 1})\n)\n",
        BuildBudget::default(),
    );
    assert_eq!(ran.printed, "1");
    assert_eq!(
        (ran.tiers.sought, ran.tiers.laid, ran.tiers.settled),
        (1, 1, 0)
    );
}

#[test]
fn an_open_callee_s_named_site_is_laid() {
    // `peek`'s name escapes, so its bindings are open, but the call passes load content alone.
    let ran = ran(
        "LET peek = (FN :{k :Holder} -> Number = #((LET v = (k :| Holder)) (7)))\n\
         PRINT (peek {k = one})\n",
        BuildBudget::default(),
    );
    assert_eq!(ran.printed, "7");
    assert_eq!(
        (ran.tiers.sought, ran.tiers.laid, ran.tiers.settled),
        (1, 1, 0)
    );
}

/// `OUTER`, traced over `ints`, whose callable `g` keys on the module it is handed by name.
const OUTER: &str = "EXPR #(OUTER m :Counter) -> Number = #((LET w = (m :| Counter)) \
                     (LET g = (FN :{k :Holder} -> Number = #((LET v = (k :| Holder)) (7)))) \
                     (g {k = one}))\n";

#[test]
fn a_callee_born_under_a_load_layer_finds_the_load_layer_beneath_it() {
    let source = format!("{OUTER}PRINT (OUTER ints)");
    let ran = ran(&source, BuildBudget::default());
    assert_eq!(ran.printed, "7");
    // `OUTER ints` finds the load's layer, and `g`'s seek the one the load laid beneath it.
    assert_eq!(
        (ran.tiers.sought, ran.tiers.laid, ran.tiers.settled),
        (2, 2, 0)
    );
    assert_eq!(ran.live, 0, "the load's layers are no depot slots");
    super::statics::loaded(&program(0, &format!("{CARRIER}{source}")), |program| {
        let parents: Vec<_> = (program.facts().layers().iter())
            .map(|laid| laid.parent)
            .collect();
        assert_eq!(
            parents,
            [None, Some(0)],
            "`g`'s layer lies beneath `OUTER`'s"
        );
    });
}

#[test]
fn a_settlement_under_a_load_layer_settles_at_run_beneath_it() {
    let ran = ran(
        &(OUTER.replace("(g {k = one})", "(g {k = (EVAL #($one) -> Holder)})")
            + "PRINT (OUTER ints)"),
        BuildBudget::default(),
    );
    assert_eq!(ran.printed, "7");
    // `OUTER ints` finds the load's layer; `g`'s seek, beneath it, misses and settles there.
    assert_eq!(
        (ran.tiers.sought, ran.tiers.laid, ran.tiers.settled),
        (2, 1, 1)
    );
    assert_eq!(ran.live, 1, "the load's layer is no depot slot");
}

#[test]
fn a_load_layer_outlives_every_eviction() {
    let ran = ran(
        &format!("PRINT (CARRIER ints)\n{EVICTING}PRINT (CARRIER ints)"),
        SMALLEST,
    );
    assert_eq!(ran.printed, "7\n7");
    // The views' misses evict each other; the second `CARRIER ints` finds the load's layer again
    // and settles nothing.
    assert_eq!((ran.tiers.laid, ran.tiers.settled), (2, 2));
    assert!(ran.tiers.evicted >= 1, "{:?}", ran.tiers);
}

/// `MAKER`, traced over `ints`, returning a callable keyed on the module it is handed.
const MAKER: &str = "EXPR #(MAKER m :Counter) -> Any = #((LET v = (m :| Counter)) \
                     (FN :{k :Holder} -> Number = #((LET w = (k :| Holder)) (7))))\n";

#[test]
fn a_callable_born_under_a_load_layer_seeks_beneath_it_and_replays_nothing() {
    LEVELS.with(|levels| levels.set(0));
    let ran = ran(
        &format!("{MAKER}LET g = (MAKER ints)\n{EVICTING}PRINT (g {{k = one}})"),
        SMALLEST,
    );
    assert_eq!(ran.printed, "7");
    // `MAKER`'s frame rests on the load's layer, which is never evicted, so it lays no level; `g`'s
    // seek misses beneath that layer, born warm with nothing to replay.
    assert_eq!(
        LEVELS.with(Cell::get),
        0,
        "no frame under a load layer lays a level"
    );
    assert_eq!(
        (ran.tiers.laid, ran.tiers.settled, ran.tiers.cold_births),
        (1, 3, 0)
    );
}

/// `NEST`, traced over `ints`, returning `g`, keyed on the module it is handed, whose frame
/// returns a callable of its own.
const NEST: &str = "EXPR #(NEST m :Counter) -> Any = #((LET v = (m :| Counter)) \
                    (FN :{k :Holder} -> Any = #((LET w = (k :| Holder)) \
                    (FN :{n :Number} -> Number = #(n + 1)))))\n";

/// What one run of `source` over [`CARRIER`] under the smallest budget printed, its tallies, and
/// how many live settlements lie directly under the root at its end.
fn under_root(source: &str) -> (String, Tiers, usize) {
    WRITTEN.with(|written| written.borrow_mut().clear());
    let program = program(0, &format!("{CARRIER}{source}"));
    let mut substrate = CellSubstrate::load::<Koan>(&program, "<test>", 2, SMALLEST, output())
        .unwrap_or_else(|error| panic!("the program loads: {error}"));
    TIERS.with(|tiers| tiers.set(Tiers::default()));
    substrate
        .with(|running| running.ran())
        .expect("the program runs");
    let rooted = substrate.with(|running| running.scheduler().graph().children_of(DepotRef::ROOT));
    let printed = WRITTEN.with(|written| written.borrow().join("\n"));
    (printed, TIERS.with(Cell::get), rooted)
}

#[test]
fn a_replay_of_a_chain_on_a_load_layer_starts_at_that_layer() {
    let (printed, tiers, rooted) = under_root(&format!(
        "{NEST}LET g = (NEST ints)\nLET h = (g {{k = one}})\n{EVICTING}PRINT (h {{n = 1}})"
    ));
    assert_eq!(printed, "2");
    // `g`'s settlement rests on the load's layer, and the views' misses evict it. `h`'s call is
    // born cold and replays its stamp: `g`'s level alone, sought again beneath the load's layer,
    // whose miss evicts the views'. A replay from the root would leave `g`'s settlement there.
    assert_eq!((tiers.laid, tiers.settled, tiers.cold_births), (1, 4, 0));
    assert_eq!(rooted, 0, "`g`'s settlement lies beneath the load's layer");
}

/// The load's refusal of `source` over [`CARRIER`], and the line and column, from 1, of the first
/// `marker` in the whole program.
fn refused(source: &str, marker: &str) -> (String, (usize, usize)) {
    let program = program(0, &format!("{CARRIER}{source}"));
    let at = program.find(marker).expect("the marker is written");
    let line = program[..at].matches('\n').count() + 1;
    let column = at - program[..at].rfind('\n').map_or(0, |newline| newline + 1) + 1;
    let refusal =
        CellSubstrate::load::<Koan>(&program, "<test>", 2, BuildBudget::default(), output())
            .err()
            .expect("the load refuses");
    (refusal.to_string(), (line, column))
}

#[test]
fn a_traced_layer_s_refusal_refuses_the_load_naming_its_call() {
    let (refusal, (line, column)) =
        refused(&format!("{TWINS}{TWIN_WRAP}PRINT (WRAP 1)\n"), "(WRAP 1)");
    let (message, call) = refusal.split_once('\n').expect("a line naming the call");
    assert_eq!(super::keys::unlocated(message), declared_refusal());
    assert_eq!(call, format!("  in (WRAP 1) at <test>:{line}:{column}"));
}

#[test]
fn a_refused_binding_names_the_first_call_in_text_order() {
    // Both calls bind `Elt := Number`: one layer, named by the call written first.
    let (refusal, (line, column)) = refused(
        &format!("{TWINS}{TWIN_WRAP}PRINT (WRAP 2)\nPRINT (WRAP 1)\n"),
        "(WRAP 2)",
    );
    let (_, call) = refusal.split_once('\n').expect("a line naming the call");
    assert_eq!(call, format!("  in (WRAP 2) at <test>:{line}:{column}"));
}

#[test]
fn the_load_table_finds_a_layer_by_its_parent_and_key() {
    let source = program(0, &format!("{CARRIER}PRINT (CARRIER ints)"));
    super::statics::loaded(&source, |program| {
        let facts = program.facts();
        let [laid] = facts.layers() else {
            panic!("the load lays one layer");
        };
        assert_eq!(laid.parent, None, "it lies beneath the root");
        let (ordinal, layer) = facts.layer(None, laid.key).expect("found beneath the root");
        assert_eq!(ordinal, 0);
        assert!(std::ptr::eq(layer, laid.layer));
        assert!(
            facts.layer(Some(0), laid.key).is_none(),
            "not beneath itself"
        );
    });
}

#[test]
fn two_traced_bindings_keeping_a_bodys_questions_share_one_witnessed_layer() {
    // `SHOW` asks nothing either binding answers otherwise: the load witnesses it once, under the
    // first, and lays each binding's layer over that one witnessed layer.
    let source = "EXPR FOR ALL #[Elt] #(SHOW x :Elt) -> Str = #((LET y = x) (\"s\"))\n\
                  PRINT (SHOW 1)\nPRINT (SHOW \"a\")\n";
    let walks = || crate::program::resume::WITNESS_WALKS.with(Cell::get);
    let before = walks();
    super::statics::loaded(&program(0, source), |program| {
        assert_eq!(walks() - before, 1, "the load witnesses once");
        let [first, second] = program.facts().layers() else {
            panic!("the load lays a binding layer per binding");
        };
        let witnessed = |laid: &crate::scope::LoadLayer<'_>| {
            let parent = laid.layer.parent().expect("over the witnessed layer");
            assert!(parent.witnessing().is_some(), "a witnessed layer");
            std::ptr::from_ref(parent).addr()
        };
        assert_eq!(witnessed(first), witnessed(second), "one witnessed layer");
    });
    let ran = ran(source, BuildBudget::default());
    assert_eq!(ran.printed, "s\ns");
    assert_eq!(
        (ran.tiers.sought, ran.tiers.laid, ran.tiers.settled),
        (2, 2, 0)
    );
}

/// The witnessing walks and the layers over a witnessed one laid loading `source` over
/// [`CARRIER`], then loading and running it, beside what the run did.
fn witnessed_counts(source: &str) -> ((usize, usize), (usize, usize), Ran) {
    use crate::program::resume::{LAYERS_LAID, WITNESS_WALKS};
    let reset = || {
        WITNESS_WALKS.with(|walks| walks.set(0));
        LAYERS_LAID.with(|laid| laid.set(0));
    };
    let counts = || (WITNESS_WALKS.with(Cell::get), LAYERS_LAID.with(Cell::get));
    reset();
    super::statics::loaded(&program(0, &format!("{CARRIER}{source}")), |_| {});
    let loaded = counts();
    reset();
    let ran = ran(source, BuildBudget::default());
    (loaded, counts(), ran)
}

#[test]
fn a_capped_bodys_callee_lays_one_layer_and_settles_its_other_bindings() {
    // `ENTRY` has a binding past the cap, so its bindings are open and `MIDDLE x` names none: the
    // load lays `MIDDLE` one layer, under `ENTRY`'s first, which witnesses `Elt`. Each other
    // binding misses at run, witnessing nothing. The witnessed layer asks one question, how
    // `LET y = x` retypes `Elt`: `Str` and `null` keep it and read the layer whole; a record
    // re-answers it, keeps no question, and walks closed.
    let values = ["1", "\"s\"", "null"].into_iter().map(String::from);
    let values = values.chain((0..CAP - 2).map(|at| format!("{{a{at} = 1}}")));
    let calls: String = values
        .map(|value| format!("PRINT (ENTRY {value})\n"))
        .collect();
    let source = format!(
        "EXPR FOR ALL #[Elt] #(MIDDLE x :Elt) -> Str = #((LET y = x) (\"m\"))\n\
         EXPR FOR ALL #[Elt] #(ENTRY x :Elt) -> Str = #(MIDDLE x)\n{calls}"
    );
    let ((walks, laid), (run_walks, run_laid), ran) = witnessed_counts(&source);
    assert_eq!(ran.printed, ["m"; CAP + 1].join("\n"));
    assert_eq!(
        walks, 2,
        "the load witnesses `ENTRY` and `MIDDLE` once each"
    );
    assert_eq!(
        laid,
        CAP + 2,
        "a layer per `ENTRY` binding, one for `MIDDLE`"
    );
    assert_eq!(run_walks, walks, "the run witnesses nothing");
    assert_eq!(
        run_laid - laid,
        2,
        "`Str` and `null` read the witnessed layer whole; no record reads over it"
    );
    assert_eq!(
        (ran.tiers.laid, ran.tiers.settled),
        (CAP + 2, CAP),
        "`MIDDLE` settles every binding but its first"
    );
}

#[test]
fn an_untraced_call_of_a_traced_body_settles_over_its_witnessed_layer() {
    // `SHOW (FLIP 1)` passes a value the trace does not follow, so it settles at run: over the
    // witnessed layer the load laid for `SHOW 1`, witnessing nothing.
    let source = "EXPR #(FLIP x :Number) -> Any = #(\"s\")\n\
                  EXPR FOR ALL #[Elt] #(SHOW x :Elt) -> Str = #((LET y = x) (\"s\"))\n\
                  PRINT (SHOW 1)\nPRINT (SHOW (FLIP 1))\n";
    let ((walks, laid), (run_walks, run_laid), ran) = witnessed_counts(source);
    assert_eq!(ran.printed, "s\ns");
    assert_eq!((walks, run_walks), (1, 1), "witnessed once, at load");
    assert_eq!(
        run_laid - laid,
        1,
        "the settlement reads over the witnessed layer"
    );
    assert_eq!((ran.tiers.laid, ran.tiers.settled), (1, 1));
}

thread_local! {
    /// The calls whose seeks computed their key from the values, and those read off a request, by
    /// their text.
    static SEEKS: RefCell<(Vec<String>, Vec<String>)> = const { RefCell::new((Vec::new(), Vec::new())) };
}

/// Note a seek that computed its key.
fn computed(birth: &KBirth<'_, '_>, _: &Sought) {
    if let KBirth::Call { call, .. } = birth {
        SEEKS.with(|seeks| seeks.borrow_mut().0.push(call.text()));
    }
}

/// Note a seek read off a request.
fn requested(birth: &KBirth<'_, '_>, _: &Sought) {
    if let KBirth::Call { call, .. } = birth {
        SEEKS.with(|seeks| seeks.borrow_mut().1.push(call.text()));
    }
}

/// [`ran`], with the calls whose seeks computed their key and those read off a request.
fn seeks(source: &str, budget: BuildBudget) -> (Ran, Vec<String>, Vec<String>) {
    SEEKS.with(|seeks| *seeks.borrow_mut() = Default::default());
    SEEKING.with(|hook| hook.set(Some(computed)));
    REQUESTING.with(|hook| hook.set(Some(requested)));
    let ran = ran(source, budget);
    SEEKING.with(|hook| hook.set(None));
    REQUESTING.with(|hook| hook.set(None));
    let (computed, requested) = SEEKS.with(|seeks| seeks.take());
    (ran, computed, requested)
}

/// The trace at the call written `text` in `source` over [`CARRIER`].
fn trace_of(source: &str, text: &str) -> String {
    super::statics::loaded(&program(0, &format!("{CARRIER}{source}")), |program| {
        format!("{:?}", super::trace::trace_at(program, text))
    })
}

/// The trace at each site of the quotes' code in `source` over [`CARRIER`].
fn quoted_traces(source: &str) -> Vec<String> {
    super::statics::loaded(&program(0, &format!("{CARRIER}{source}")), |program| {
        (program.shapes().bodies().iter())
            .filter(|shape| shape.kind() == crate::scope::ShapeKind::Code)
            .flat_map(|code| {
                (program.facts().traces(code).iter())
                    .map(|(_, trace)| format!("{trace:?}"))
                    .collect::<Vec<_>>()
            })
            .collect()
    })
}

#[test]
fn a_traced_call_reads_its_request_and_computes_no_key() {
    let (ran, computed, requested) = seeks(
        "PRINT (CARRIER ints)\nPRINT (CARRIER (ints :| Counter))",
        BuildBudget::default(),
    );
    assert_eq!(ran.printed, "7\n7");
    assert_eq!(computed, [""; 0]);
    assert_eq!(requested, ["(CARRIER ints)", "(CARRIER (ints :| Counter))"]);
    assert_eq!(
        (ran.tiers.sought, ran.tiers.laid, ran.tiers.settled),
        (2, 2, 0)
    );
}

#[test]
fn a_site_passing_a_slot_reads_its_request_under_a_settlement() {
    // `PASSER`'s call is untraced, so its frame runs under a settlement, whose layer carries the
    // request of the site passing `m` on.
    let (ran, computed, requested) = seeks(
        "EXPR #(PASSER m :Counter) -> Number = #((LET w = (m :| Counter)) (CARRIER m))\n\
         PRINT (PASSER (EVAL #($ints) -> Counter))",
        BuildBudget::default(),
    );
    assert_eq!(ran.printed, "7");
    assert_eq!(computed, ["(PASSER (EVAL #($ints) -> Counter))"]);
    assert_eq!(requested, ["(CARRIER m)"]);
}

#[test]
fn a_callee_born_off_the_chain_computes_its_key_from_the_values() {
    // `g` is a call's result, which the walk traces no call of: its site carries no request.
    let (ran, computed, requested) = seeks(
        &format!("{MAKER}LET g = (MAKER ints)\nPRINT (g {{k = one}})"),
        BuildBudget::default(),
    );
    assert_eq!(ran.printed, "7");
    assert_eq!(computed, ["(g {k = one})"]);
    assert_eq!(requested, ["(MAKER ints)"]);
    // `g`'s seek misses beneath `MAKER`'s load layer and settles there.
    assert_eq!(
        (ran.tiers.sought, ran.tiers.laid, ran.tiers.settled),
        (2, 1, 1)
    );
}

/// `MAKE`, keyed on the module it views, returning a lambda that calls `INNER`, which `MAKE`
/// declares and which keys on the module it views.
const MAKE: &str = "EXPR #(MAKE m :Counter) -> Any = #((LET w = (m :| Counter)) \
                    (EXPR #(INNER k :Holder) -> Number = #((LET v = (k :| Holder)) (7))) \
                    (FN :{} -> Number = #(INNER one)))\n";

#[test]
fn an_evicted_parent_replays_without_computing_a_key() {
    let source =
        format!("{MAKE}LET l = (MAKE (EVAL #($ints) -> Counter))\n{EVICTING}PRINT (l {{}})");
    let trace = trace_of(&source, "(INNER one)");
    assert!(trace.starts_with("Named"), "`INNER one` is named: {trace}");
    let (ran, computed, requested) = seeks(&source, SMALLEST);
    assert_eq!(ran.printed, "7");
    // `MAKE`'s call is untraced, so its frame runs under a settlement the views' misses evict;
    // `(l {})` replays it, and `INNER`'s closure still names the evicted slot, so its call is born
    // cold, handed its request, and its replay seeks with it.
    assert!(
        !computed.iter().any(|call| call == "(INNER one)"),
        "{computed:?}"
    );
    assert_eq!(
        (computed.iter())
            .filter(|call| call.starts_with("(MAKE"))
            .count(),
        1
    );
    assert_eq!(requested, ["(INNER one)"]);
    assert!(ran.tiers.evicted >= 1, "{:?}", ran.tiers);
    assert_eq!(ran.tiers.cold_births, 0);
}

#[test]
fn an_untraced_site_computes_its_key_from_the_values() {
    let routes = [
        (
            "PRINT (CARRIER (EVAL #($ints) -> Counter))",
            "(CARRIER (EVAL #($ints) -> Counter))",
        ),
        (
            "LET seen = (ints :| Counter)\n\
             EXPR #(VIEWER m :Counter) -> Number = #(CARRIER (m :| Counter))\n\
             PRINT (VIEWER ints)",
            "(CARRIER (m :| Counter))",
        ),
        (
            "LET carry = (FN :{m :Counter} -> Number = #((LET v = (m :| Counter)) (7)))\n\
             PRINT (EVAL #($carry {m = $ints}) -> Number)",
            "($carry {m = $ints})",
        ),
    ];
    for (source, call) in routes {
        // A quote's code is traced nowhere: each of its sites the walk records is composed.
        match source.contains("EVAL #($carry") {
            true => {
                let traces = quoted_traces(source);
                assert!(
                    traces.iter().all(|trace| trace == "Untraced(Composed)"),
                    "{traces:?}"
                );
            }
            false => {
                let trace = trace_of(source, call);
                assert!(
                    trace.starts_with("Untraced"),
                    "`{call}` is untraced: {trace}"
                );
            }
        }
        let (ran, computed, requested) = seeks(source, BuildBudget::default());
        assert_eq!(ran.printed, "7", "{source}");
        assert!(
            computed.iter().any(|each| each == call),
            "{source}: {computed:?}"
        );
        assert!(
            !requested.iter().any(|each| each == call),
            "{source}: {requested:?}"
        );
    }
}

#[test]
fn a_request_and_a_key_from_the_values_find_one_layer() {
    let ran = ran(
        "PRINT (CARRIER (ints :| Counter))\nPRINT (CARRIER (EVAL #($ints :| $Counter) -> Counter))",
        BuildBudget::default(),
    );
    assert_eq!(ran.printed, "7\n7");
    assert_eq!((ran.tiers.laid, ran.tiers.settled), (2, 0));
}

/// `PASS`, which reads nothing of `m` and hands it on to [`CARRIER`].
const PASS: &str = "EXPR #(PASS m :Counter) -> Number = #(CARRIER m)\n";

#[test]
fn a_pass_through_body_s_call_into_a_keyed_callee_computes_no_key() {
    let source = format!("{PASS}PRINT (PASS ints)");
    let (ran, computed, requested) = seeks(&source, BuildBudget::default());
    assert_eq!(ran.printed, "7");
    assert_eq!(computed, [""; 0]);
    assert_eq!(requested, ["(PASS ints)", "(CARRIER m)"]);
    assert_eq!(
        (ran.tiers.sought, ran.tiers.laid, ran.tiers.settled),
        (2, 2, 0)
    );
    super::statics::loaded(&program(0, &format!("{CARRIER}{source}")), |program| {
        let parents: Vec<_> = (program.facts().layers().iter())
            .map(|laid| laid.parent)
            .collect();
        assert_eq!(
            parents,
            [None, None],
            "`PASS`'s layer and `CARRIER`'s, beneath the root"
        );
    });
}

/// `MAKESET`, a functor over an `Ordered` module, and `FRESH`, which hands its own on to it, as the
/// tutorial writes them.
const FRESH: &str = "SIG Ordered = #[(VAL compare :Number)]
SIG Collection FOR ALL #[Set] = #[(VAL empty :Set)]
MODULE up = (LET compare = 1)
EXPR #(MAKESET elem :Ordered) -> Module = #(
  (MODULE built OVER #[elem] = (LET empty = [elem.compare]))
  (built :| Collection)
)
EXPR #(FRESH elem :Ordered) -> Any = #(
  (LET made = (MAKESET elem))
  (LET kept :(made.Set) = made.empty)
  kept
)
";

#[test]
fn a_pass_through_chain_lays_each_layer_once() {
    // `FRESH`'s frame binds `up` at `Ordered`, as `MAKESET`'s own call does: one `MAKESET` layer.
    let source = format!("{FRESH}PRINT (FRESH up)\nLET same = (MAKESET up)\n");
    let ran = ran(&source, BuildBudget::default());
    assert_eq!(ran.printed, "Set([1])");
    assert_eq!(
        (ran.tiers.sought, ran.tiers.laid, ran.tiers.settled),
        (3, 3, 0)
    );
    super::statics::loaded(&program(0, &format!("{CARRIER}{source}")), |program| {
        assert_eq!(
            program.facts().layers().len(),
            2,
            "one `FRESH` layer, one `MAKESET` layer"
        );
    });
}

thread_local! {
    /// Each binding event's site, and whether it walked.
    static EVENTS: RefCell<Vec<(EventSite, bool)>> = const { RefCell::new(Vec::new()) };
}

/// Note a binding event.
fn decided(site: EventSite, walked: bool) {
    EVENTS.with(|events| events.borrow_mut().push((site, walked)));
}

#[test]
fn run_content_at_a_traced_slot_seeks_no_settlement() {
    // `TWO` keys on `n`, which it views, and on `m`, which it hands on to `CARRIER`. `HOST` passes
    // it a module built over its parameter at run: `TWO`'s call seeks nothing and its frame
    // decides in its activation, so `CARRIER`'s call, from a frame under no layer of `TWO`, finds
    // no request and computes its key.
    let source = "EXPR #(TWO m :Counter n :Holder) -> Number = \
                  #((LET v = (n :| Holder)) (CARRIER m))\n\
                  EXPR #(HOST x :Number) -> Number = \
                  #((MODULE b OVER #[x] = (LET held = x)) (TWO ints b))\n\
                  PRINT (HOST 1)";
    let two = super::statics::loaded(&program(0, &format!("{CARRIER}{source}")), |program| {
        let mut shapes = Vec::new();
        super::caching::every_shape(program.shape(), &mut shapes);
        (shapes.into_iter())
            .find(|shape| {
                shape.kind() == ShapeKind::Callable
                    && (0..shape.slots()).any(|at| {
                        let name = shape.slot_name(Slot(at as u32)).symbol();
                        program.symbols().display(name).to_string() == "n"
                    })
            })
            .expect("`TWO` is written")
            .segment()
    });
    EVENTS.with(|events| events.borrow_mut().clear());
    DECIDING.with(|hook| hook.set(Some(decided)));
    let (ran, computed, requested) = seeks(source, BuildBudget::default());
    DECIDING.with(|hook| hook.set(None));
    assert_eq!(ran.printed, "7");
    assert_eq!(computed, ["(CARRIER m)"], "`TWO`'s call seeks nothing");
    assert_eq!(requested, [""; 0]);
    let events = EVENTS.with(|events| events.take());
    assert!(
        events.contains(&(EventSite::entry(two), true)),
        "`TWO`'s entry walks in its activation: {events:?}"
    );
}

thread_local! {
    /// The drain's tallies before and after each keyed seek.
    static ANSWERS: RefCell<Vec<(Tiers, Tiers)>> = const { RefCell::new(Vec::new()) };
}

/// Note a keyed seek's tallies.
fn answered(_: LayerKey, before: Tiers, after: Tiers) {
    ANSWERS.with(|answers| answers.borrow_mut().push((before, after)));
}

#[test]
fn a_miss_lays_its_own_layer_and_a_child_at_its_first_call() {
    // `PASS2`'s call is untraced, so it misses and lays its one layer, its requests with it but no
    // child: the first `CARRIER` call lays `CARRIER`'s, and the second finds it.
    let source = "EXPR #(PASS2 m :Counter) -> Number = \
                  #((LET a = (CARRIER m)) (LET b = (CARRIER m)) (b))\n\
                  PRINT (PASS2 (EVAL #($ints) -> Counter))";
    ANSWERS.with(|answers| answers.borrow_mut().clear());
    ANSWERED.with(|hook| hook.set(Some(answered)));
    let (ran, _, requested) = seeks(source, BuildBudget::default());
    ANSWERED.with(|hook| hook.set(None));
    assert_eq!(ran.printed, "7");
    assert_eq!(requested, ["(CARRIER m)", "(CARRIER m)"]);
    let settled: Vec<_> = (ANSWERS.with(|answers| answers.take()).into_iter())
        .map(|(before, after)| (after.settled - before.settled, after.laid - before.laid))
        .collect();
    assert_eq!(
        settled,
        [(1, 0), (1, 0), (0, 0)],
        "`PASS2` lays one layer, the first `CARRIER` call one, and the second finds it"
    );
}

#[test]
fn a_refusal_in_a_layer_a_miss_lays_faults_the_call() {
    // `HOLD`'s call is untraced, so its layer is laid at run with `WRAP`'s request at
    // `Elt := Number`; `WRAP`'s call reads it and misses, and that layer's refusal faults it.
    let source = format!(
        "{TWINS}{TWIN_WRAP}EXPR FOR ALL #[Elt] #(HOLD x :Elt) -> Any = #(WRAP x)\n\
         PRINT (HOLD (EVAL #(1) -> Any))"
    );
    let (ran, _, requested) = seeks(&source, BuildBudget::default());
    let printed = (ran.printed.lines())
        .find(|line| !line.starts_with("  in "))
        .expect("the run prints");
    assert_eq!(super::keys::fault(printed), declared_refusal());
    assert!(requested.contains(&"(WRAP x)".to_owned()), "{requested:?}");
}

#[test]
fn every_layer_carries_the_requests_of_its_covered_sites() {
    use crate::scope::{Facts, Site};
    // `PASS`'s body calls `CARRIER` over load content and over its own `m`; a quote's code calls
    // `carry`.
    let source = program(
        0,
        &format!(
            "{CARRIER}EXPR #(PASS m :Counter) -> Number = #((LET a = (CARRIER ints)) (CARRIER m))\n\
             LET carry = (FN :{{m :Counter}} -> Number = #((LET v = (m :| Counter)) (7)))\n\
             PRINT (PASS ints)\nPRINT (PASS (ints :| Counter))\n\
             PRINT (EVAL #($carry {{m = $ints}}) -> Number)"
        ),
    );
    let mut substrate =
        CellSubstrate::load::<Koan>(&source, "<test>", 8, BuildBudget::default(), output())
            .unwrap_or_else(|error| panic!("the program loads: {error}"));
    substrate.with(|running| {
        let program = running.program();
        let (shapes, load) = (program.shapes(), program.facts());
        // The one traced site whose call is written `text`.
        let site = |text: &str| -> (&crate::scope::BodyShape<'_>, Site) {
            let found: Vec<_> = (shapes.bodies().iter())
                .flat_map(|shape| {
                    let traces = load.traces(shape);
                    (traces.iter())
                        .filter(|(site, _)| {
                            super::trace::call_of(shape, *site)
                                .is_some_and(|call| call.text() == text)
                        })
                        .map(|(site, _)| (*shape, *site))
                        .collect::<Vec<_>>()
                })
                .collect();
            match found[..] {
                [at] => at,
                _ => panic!("`{text}` is traced once: {}", found.len()),
            }
        };
        let request = |layer: &crate::scope::BuildLayer<'_>,
                       (shape, at): (&crate::scope::BodyShape<'_>, Site)| {
            layer.request(shape.segment(), at).map(|each| each.sought())
        };
        let (pass, view) = (site("(PASS ints)"), site("(PASS (ints :| Counter))"));
        let (inner, passed) = (site("(CARRIER ints)"), site("(CARRIER m)"));
        // The base: each site passing load content alone, in whatever body.
        let based = |at: (&crate::scope::BodyShape<'_>, Site)| load.request(at.0, at.1);
        assert!(based(pass).is_some() && based(inner).is_some());
        assert!(based(passed).is_none());
        // `PASS`'s load layer: the site passing its own `m`, and no other.
        let key = based(pass).expect("`(PASS ints)` is requested").key;
        let laid = (load.layers().iter())
            .find(|laid| laid.parent.is_none() && laid.key == key)
            .expect("the load lays `(PASS ints)`'s layer");
        assert!(request(laid.layer, passed).is_some());
        assert!(request(laid.layer, inner).is_none());
        // A settlement of `PASS` under the view's binding: the same site's request.
        let bump = crate::memory::Bump::new();
        let ask = based(view)
            .expect("the view's call is requested")
            .sought()
            .ask;
        let settled = running
            .settle::<Koan>(&ask, &bump)
            .expect("`ints` has a tree in the load");
        let top = settled.top().expect("a settlement is its facts' top layer");
        assert!(request(top, passed).is_some());
        // A quote's code: no request on any layer.
        let mut quoted = 0;
        for code in (shapes.bodies().iter()).filter(|shape| shape.kind() == ShapeKind::Code) {
            for (at, _) in load.traces(code).iter() {
                quoted += 1;
                let at = (*code, *at);
                assert!(based(at).is_none() && request(top, at).is_none());
                assert!(
                    load.layers()
                        .iter()
                        .all(|laid| request(laid.layer, at).is_none())
                );
                assert!(Facts::under(load, Some(top)).request(at.0, at.1).is_none());
            }
        }
        assert!(quoted > 0, "the quote's code holds a traced site");
    });
}

/// Every program above whose seeks read a request, named, over [`CARRIER`]: what the cross-check law
/// runs beside the tutorial.
pub(super) fn fixtures() -> Vec<(String, String)> {
    let sources = [
        format!("{BOTH}PRINT (BOTH 1 AND 2)"),
        "PRINT (CARRIER ints)\nPRINT (CARRIER (ints :| Counter))".to_owned(),
        "PRINT (CARRIER (ints :! Counter))".to_owned(),
        format!("{OUTER}PRINT (OUTER ints)"),
        format!("{MAKER}LET g = (MAKER ints)\nPRINT (g {{k = one}})"),
        format!(
            "{NEST}LET g = (NEST ints)\nLET h = (g {{k = one}})\n{EVICTING}PRINT (h {{n = 1}})"
        ),
        format!("{MAKE}LET l = (MAKE (EVAL #($ints) -> Counter))\n{EVICTING}PRINT (l {{}})"),
        "EXPR #(PASSER m :Counter) -> Number = #((LET w = (m :| Counter)) (CARRIER m))\n\
         PRINT (PASSER (EVAL #($ints) -> Counter))"
            .to_owned(),
        "LET peek = (FN :{k :Holder} -> Number = #((LET v = (k :| Holder)) (7)))\n\
         PRINT (peek {k = one})"
            .to_owned(),
        format!("{PASS}PRINT (PASS ints)"),
        format!("{FRESH}PRINT (FRESH up)\nLET same = (MAKESET up)\n"),
    ];
    (sources.into_iter())
        .map(|source| (source.clone(), program(0, &format!("{CARRIER}{source}"))))
        .collect()
}
