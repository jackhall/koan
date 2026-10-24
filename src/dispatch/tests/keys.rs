//! Settlement keys over frontiers: which names a callable's key lists, folded from the
//! frontiers of the units beneath it, and what a call of it seeks at run.

use crate::parse::ExpressionPart;
use crate::program::Program;
use crate::scope::{
    BodyShape, Mint, Narrowing, Segment, ShapeKind, Site, Slot, UnitIndex, Unknown,
};

use super::caching::{PRELUDE, Ran, SMALLEST, every_shape, minted, program, ran_under};
use super::frontiers::{quantified, seam, unknown};
use super::statics::{body, loaded, slot};
use crate::values::BuildBudget;

/// Two overloads of `PICK`, one at `Number` and one at `Str`.
const PICK: &str = "EXPR #(PICK x :Number) -> Str = #(\"n\")\n\
                    EXPR #(PICK x :Str) -> Number = #(1)\n";

/// `SAME`, which joins its two operands' types under one `FOR ALL` name.
const SAME: &str = "EXPR FOR ALL #[Elt] #(SAME a :Elt AND b :Elt) -> Elt = #(a)\n";

/// The names `shape`'s slots bind, in slot order.
fn names(program: &Program<'_>, shape: &BodyShape<'_>) -> Vec<String> {
    (0..shape.slots())
        .map(|index| name(program, shape, Slot(index as u32)))
        .collect()
}

/// The name `shape` binds at `slot`.
fn name(program: &Program<'_>, shape: &BodyShape<'_>, slot: Slot) -> String {
    let symbol = shape.slot_name(slot).symbol();
    program.symbols().display(symbol).to_string()
}

/// The names `shape`'s key lists, in slot order: `None` where a call of it seeks nothing.
fn listed(program: &Program<'_>, shape: &BodyShape<'_>) -> Option<Vec<String>> {
    (program.facts().settlement_keys(shape)).map(|slots| {
        slots
            .iter()
            .map(|slot| name(program, shape, *slot))
            .collect()
    })
}

/// Each callable of `source`, by the names its slots bind, beside the names its key lists.
fn keys(source: &str) -> Vec<(Vec<String>, Option<Vec<String>>)> {
    loaded(source, |program| {
        let mut shapes = Vec::new();
        every_shape(program.shape(), &mut shapes);
        (shapes.into_iter())
            .filter(|shape| shape.kind() == ShapeKind::Callable)
            .map(|shape| (names(program, shape), listed(program, shape)))
            .collect()
    })
}

/// What the key of the one callable of `source` whose slots bind `slots` lists.
fn keyed(source: &str, slots: &[&str]) -> Option<Vec<String>> {
    let found = (keys(source).into_iter()).find(|(names, _)| *names == slots);
    found
        .unwrap_or_else(|| panic!("a callable binds {slots:?}: {:?}", keys(source)))
        .1
}

/// `names` as owned strings.
fn some(names: &[&str]) -> Option<Vec<String>> {
    Some(names.iter().map(|name| name.to_string()).collect())
}

/// Whether some unit of `shape` waits on every one of `unknowns` and seams at `shape`'s entry.
fn seams_at_entry_over(program: &Program<'_>, shape: &BodyShape<'_>, unknowns: &[Unknown]) -> bool {
    (program.facts().own_units().iter())
        .filter(|(unit, (_, frontier))| {
            unit.segment == shape.segment()
                && unknowns
                    .iter()
                    .all(|unknown| frontier.unknowns().contains(unknown))
        })
        .any(|(unit, _)| {
            seam(program, *unit).is_some_and(|site| {
                (site.segment, site.unit) == (shape.segment(), UnitIndex::ENTRY)
            })
        })
}

#[test]
fn a_body_keys_on_a_for_all_name_a_frontier_at_its_entry_waits_on() {
    let source =
        format!("{PICK}EXPR FOR ALL #[Elt] #(WRAP x :Elt) -> Any = #(\n  LET p = (PICK x)\n  p\n)");
    loaded(&source, |program| {
        let wrap = quantified(program);
        assert_eq!(listed(program, wrap), some(&["Elt"]));
        assert!(
            (program.facts().caching(wrap)).is_some_and(|caching| caching.invariant.is_none()),
            "`WRAP` keys through its frontier alone"
        );
    });
    let closed =
        format!("{PICK}LET f = (FN :{{x :Number}} -> Any = #(\n  LET p = (PICK x)\n  p\n))");
    loaded(&closed, |program| {
        let f = body(program, program.shape(), "f");
        assert_eq!(listed(program, f), None, "a closed body keys nothing");
    });
}

#[test]
fn a_body_keys_on_a_module_parameter_a_path_through_it_waits_on() {
    let source = format!(
        "{PRELUDE}{SAME}LET step = (FN :{{m :Counter}} -> Any = \
         #(LET s = (SAME m.zero AND (m.succ {{x = m.zero}}))))\n"
    );
    loaded(&source, |program| {
        let step = body(program, program.shape(), "step");
        assert_eq!(listed(program, step), some(&["m"]));
    });
}

#[test]
fn a_body_keys_on_its_own_names_a_nested_body_s_entry_frontier_waits_on() {
    // `PAIRED`'s unit waits on `OUTER`'s `m` and its own `Inner`, so it seams at `PAIRED`'s entry:
    // `OUTER` keys `m`, and `PAIRED`, whose settlement lies beneath `OUTER`'s, `Inner`.
    let source = format!(
        "{PRELUDE}{SAME}EXPR #(OUTER m :Counter) -> Any = #((EXPR FOR ALL #[Inner] \
         #(PAIRED x :Inner) -> Any = #(SAME m.zero AND x)) (PAIRED 1))\n"
    );
    loaded(&source, |program| {
        let mut shapes = Vec::new();
        every_shape(program.shape(), &mut shapes);
        let find = |slots: &[&str]| {
            *(shapes.iter())
                .find(|shape| names(program, shape) == slots)
                .unwrap_or_else(|| panic!("a callable binds {slots:?}"))
        };
        let outer = find(&["m", "<symbol>"]);
        let paired = find(&["x", "Inner"]);
        let (m, inner) = (
            unknown(program, outer, "m"),
            unknown(program, paired, "Inner"),
        );
        assert!(
            seams_at_entry_over(program, paired, &[m, inner]),
            "a unit of `PAIRED` waits on `m` and `Inner` and seams at its entry"
        );
        assert_eq!(listed(program, outer), some(&["m"]));
        assert_eq!(listed(program, paired), some(&["Inner"]));
    });
    // Without `PAIRED`, only `m`'s own type reads `m`, at the entry, which keys nothing.
    let alone = format!("{PRELUDE}EXPR #(OUTER m :Counter) -> Any = #(1)\n");
    assert_eq!(keyed(&alone, &["m"]), None);
}

#[test]
fn a_capture_keys_the_body_that_binds_it_never_the_one_reading_it() {
    let source = format!(
        "{PICK}EXPR FOR ALL #[Elt] #(WRAP x :Elt) -> Any = #(\n  \
         LET g = (FN :{{}} -> Any = #(\n    LET p = (PICK x)\n    p\n  ))\n  g\n)"
    );
    loaded(&source, |program| {
        let wrap = quantified(program);
        let (_, g) = (wrap.nested_shapes().iter())
            .find(|(_, nested)| nested.kind() == ShapeKind::Callable)
            .expect("`g` is nested in `WRAP`");
        assert_eq!(listed(program, wrap), some(&["Elt"]));
        assert_eq!(listed(program, g), None);
    });
}

#[test]
fn a_frontier_name_whose_parameter_does_not_key_keys_nothing() {
    // `m`'s declared type is open, so its digest is no constant cost.
    let source = format!(
        "{PICK}SIG Pair FOR ALL #[Left Right] = #[(VAL l :Left) (VAL r :Right)]\n\
         EXPR FOR ALL #[Elt] #(USE m :(Pair WITH {{Left = Elt}})) -> Any = #(LET p = (PICK m.r))\n"
    );
    loaded(&source, |program| {
        let using = quantified(program);
        let m = unknown(program, using, "m");
        assert!(
            seams_at_entry_over(program, using, &[m]),
            "a unit of `USE` waits on `m` and seams at its entry"
        );
        let listed = listed(program, using).unwrap_or_default();
        assert!(
            !listed.contains(&"m".to_string()),
            "`m` keys nothing: {listed:?}"
        );
    });
}

/// `SWAP`, a loop over `Link` whose recursive arm reads a path through `m` and hands `passed` on
/// as its next `m` and `k`.
fn swapping(passed: &str) -> String {
    format!(
        "{PRELUDE}{PICK}EXPR #(SWAP m :Counter k :Counter link :Link) -> Any = \
         #((LET p = (PICK m.zero)) (SWAP {passed} (ATTR link next)))\n\
         EXPR #(SWAP m :Counter k :Counter end :Null) -> Any = #(1)\n"
    )
}

#[test]
fn a_name_the_body_s_own_tail_rebinds_is_not_keyed() {
    let arm = ["p", "link", "m", "k"];
    assert_eq!(keyed(&swapping("k m"), &arm), None, "a swap rebinds both");
    assert_eq!(keyed(&swapping("m k"), &arm), some(&["m", "k"]));
    assert_eq!(
        keyed(&swapping("m m"), &arm),
        some(&["m"]),
        "only `k` is rebound"
    );
}

#[test]
fn a_swapping_loop_s_view_routes_cold() {
    let flip = |passed: &str| {
        format!(
            "{PRELUDE}EXPR #(FLIP m :Counter k :Counter link :Link) -> Number = \
             #((LET v = (m :| Counter)) (FLIP {passed} (ATTR link next)))\n\
             EXPR #(FLIP m :Counter end :Null k :Counter) -> Number = #(7)\n"
        )
    };
    let arm = ["link", "v", "m", "k"].map(str::to_string).to_vec();
    let mints = |source: &str| {
        (minted(source).into_iter())
            .find(|(names, _)| *names == arm)
            .map(|(_, mints)| mints)
    };
    assert_eq!(mints(&flip("k m")), Some(vec![Mint::Varying]));
    assert_eq!(keyed(&flip("k m"), &["link", "v", "m", "k"]), None);
    assert_eq!(mints(&flip("m k")), Some(vec![Mint::Invariant]));
    assert_eq!(
        keyed(&flip("m k"), &["link", "v", "m", "k"]),
        some(&["m", "k"])
    );
}

/// [`ran_under`] at depth 2 with roomy tables.
fn ran(source: &str) -> Ran {
    ran_under(2, source, BuildBudget::default())
}

/// `source`'s run's seeks and builds.
fn sought(source: &str) -> (usize, usize, String) {
    let ran = ran(source);
    (ran.tiers.sought, ran.tiers.settled, ran.printed)
}

/// `WRAP`, keyed on its `FOR ALL` name alone, at `Shares` and at `Fresh`.
const WRAPS: [&str; 2] = [
    "EXPR FOR ALL #[Elt] #(WRAP x :Elt) -> Any = #((LET p = (PICK x)) (p))\n",
    "EXPR FOR ALL #[Elt] #(WRAP x :Elt) -> Number = #((LET p = (PICK x)) (1))\n",
];

#[test]
fn two_calls_differing_at_a_frontier_name_build_two_settlements_and_agreeing_ones_share() {
    for (wrap, printed) in WRAPS.into_iter().zip([["n\nn", "n\n1"], ["1\n1", "1\n1"]]) {
        let agree = format!("{PICK}{wrap}PRINT (WRAP 1)\nPRINT (WRAP 2)\n");
        assert_eq!(sought(&agree), (2, 1, printed[0].to_string()), "{wrap}");
        let differ = format!("{PICK}{wrap}PRINT (WRAP 1)\nPRINT (WRAP \"s\")\n");
        assert_eq!(sought(&differ), (2, 2, printed[1].to_string()), "{wrap}");
    }
}

#[test]
fn two_calls_differing_at_a_module_parameter_build_two_settlements() {
    // `SAME`, called once a `step`, keys its own `Elt` on the carrier `m` passes it.
    let step = "LET step = (FN :{m :Counter} -> Any = \
                #((LET s = (SAME m.zero AND (m.succ {x = m.zero}))) (s)))\n";
    let agree = format!("{SAME}{step}PRINT (step {{m = ints}})\nPRINT (step {{m = ints}})\n");
    assert_eq!(sought(&agree), (4, 2, "0\n0".to_string()));
    let differ = format!("{SAME}{step}PRINT (step {{m = ints}})\nPRINT (step {{m = words}})\n");
    assert_eq!(sought(&differ), (4, 4, "0\n".to_string()));
}

#[test]
fn an_evicted_settlement_costs_a_recomputation_and_never_a_different_answer() {
    let source = format!(
        "{PICK}{}PRINT (WRAP 1)\nPRINT (WRAP \"s\")\nPRINT (WRAP 1)\n",
        WRAPS[0]
    );
    let tight = ran_under(2, &source, SMALLEST);
    let roomy = ran(&source);
    assert_eq!(tight.printed, "n\n1\nn");
    assert_eq!(roomy.printed, tight.printed);
    assert_eq!((tight.tiers.settled, tight.tiers.evicted), (3, 2));
    assert_eq!((roomy.tiers.settled, roomy.tiers.evicted), (2, 0));
}

/// `pick`, a quantified callable keyed on its `FOR ALL` name, as a module's member: a call by
/// name of it carries no solution, so the asker solves it.
const LIB: &str = "MODULE lib = (LET pick = (FN FOR ALL #[Elt] :{x :Elt} -> Any = \
                   #((LET p = (PICK x)) (p))))\n";

#[test]
fn a_call_by_name_of_a_quantified_body_keys_on_its_solution() {
    let calls = |first: &str, second: &str| {
        format!("{PICK}{LIB}PRINT (lib.pick {{x = {first}}})\nPRINT (lib.pick {{x = {second}}})\n")
    };
    assert_eq!(sought(&calls("1", "2")), (2, 1, "n\nn".to_string()));
    assert_eq!(sought(&calls("1", "\"s\"")), (2, 2, "n\n1".to_string()));
}

#[test]
fn an_instance_keys_on_the_solution_it_holds() {
    // `lib.pick` handed where a closed function type is wanted is an instance at that type's
    // parameter; `f` keys nothing, so only `pick` seeks.
    let source = |second: &str| {
        format!(
            "{PICK}{LIB}EXPR #(NUMBERED f :(FN :{{x :Number}} -> Any)) -> Any = #(f {{x = 1}})\n\
             EXPR #(WORDED f :(FN :{{x :Str}} -> Any)) -> Any = #(f {{x = \"s\"}})\n\
             PRINT (NUMBERED lib.pick)\nPRINT ({second} lib.pick)\n"
        )
    };
    assert_eq!(sought(&source("NUMBERED")), (2, 1, "n\nn".to_string()));
    assert_eq!(sought(&source("WORDED")), (2, 2, "n\n1".to_string()));
}

#[test]
fn past_a_header_or_a_barrier_a_body_keyed_on_a_for_all_name_is_born_cold() {
    // A header: `Picker` shows `pick` returning `Any`, where it declares `Str`.
    let header = "MODULE lib = (LET pick = (FN FOR ALL #[Elt] :{x :Elt} -> Str = \
                  #((LET p = (PICK x)) (\"s\"))))\n\
                  SIG Picker = #[(VAL pick :(FN FOR ALL #[Elt] :{x :Elt} -> Any))]\n";
    // A barrier: the view hides `Carrier`, which `pick`'s type reads.
    let barrier = "MODULE lib = ((LET zero = 0) (LET pick = (FN FOR ALL #[Elt] \
                   :{x :Elt c :Number} -> Any = #((LET p = (PICK x)) (p)))))\n\
                   SIG Picker FOR ALL #[Carrier] = #[(VAL zero :Carrier) \
                   (VAL pick :(FN FOR ALL #[Elt] :{x :Elt c :Carrier} -> Any))]\n";
    for (lib, direct, viewed) in [
        (header, "lib.pick {x = 1}", "viewed.pick {x = 1}"),
        (
            barrier,
            "lib.pick {x = 1, c = 0}",
            "viewed.pick {x = 1, c = viewed.zero}",
        ),
    ] {
        let seen = |call: &str| {
            sought(&format!(
                "{PICK}{lib}LET viewed = (lib :| Picker)\nPRINT ({call})\n"
            ))
        };
        let (keyed, built, printed) = seen(direct);
        assert_eq!((keyed, built), (1, 1), "{direct}");
        assert_eq!(seen(viewed), (0, 0, printed), "{viewed}");
    }
}

#[test]
fn an_escaping_body_s_hop_seeks_with_its_own_content() {
    // `hop`'s name escapes into `CHOOSE`, so it reaches its second and third frames through
    // `next`, a value the load cannot follow, passing `words` where the first frame bound `ints`:
    // each hop seeks. `done`, which reads nothing, keys nothing.
    let source = format!(
        "{SAME}EXPR #(CHOOSE link :Link f :Any g :Any) -> Any = #(f)\n\
         EXPR #(CHOOSE end :Null f :Any g :Any) -> Any = #(g)\n\
         EXPR #(STEPPED link :Link) -> Any = #(ATTR link next)\n\
         EXPR #(STEPPED end :Null) -> Any = #(null)\n\
         LET done = (FN :{{m :Counter link :Any}} -> Number = #(7))\n\
         LET hop = (FN :{{m :Counter link :Any}} -> Number = #(\n  \
         LET s = (SAME m.zero AND (m.succ {{x = m.zero}}))\n  \
         LET next = (CHOOSE link hop done)\n  \
         next {{m = words, link = (STEPPED link)}}\n))\n\
         PRINT (hop {{m = ints, link = chain}})\n"
    );
    let shapes = keys(&format!("{PRELUDE}LET chain = null\n{source}"));
    let hop = (shapes.iter()).find(|(names, _)| names.contains(&"next".to_string()));
    assert_eq!(hop.map(|(_, keys)| keys.clone()), Some(some(&["m"])));
    // Three `hop` frames at `ints`, `words` and `words`, and `SAME`'s calls in each, one at
    // `Number` and two at `Str`.
    let ran = ran(&source);
    assert_eq!(ran.printed, "7");
    assert_eq!((ran.tiers.sought, ran.tiers.settled), (6, 4));
}

/// A unit that reads an entry-bound name beside a later-bound one seams at the later one, so its
/// callable keys nothing through it (F4): `s` reads `x`, bound at the entry, and `n`, bound at its
/// own unit. `n` comes from a top-level maker, so no other unit seams at the entry.
#[test]
fn a_unit_mixing_seams_keys_nothing_at_the_entry() {
    let source = format!(
        "{PRELUDE}{PICK}LET maker = (FN :{{}} -> Counter = #(ints))\n\
         EXPR FOR ALL #[Elt] #(SUBJECT x :Elt) -> Any = #(\n  \
         LET n = (maker {{}})\n  LET s = [(PICK x), n.zero]\n  s\n)\n"
    );
    assert_eq!(keyed(&source, &["n", "x", "s", "Elt"]), None);
}

/// A unit that reads a parameter waits on every name its entry's frontier holds, so its callable
/// keys on each keyable one: `p` reads `x` alone, and `SUBJECT` keys `Key` beside `Elt`.
#[test]
fn a_callable_keys_on_each_keyable_name_its_entry_holds_that_a_unit_reads_through() {
    let source = format!(
        "{PICK}EXPR FOR ALL #[Elt Key] #(SUBJECT x :Elt AND k :Key) -> Any = #(\n  \
         LET p = (PICK x)\n  p\n)\n"
    );
    let found = keys(&source);
    let (_, listed) = (found.iter())
        .find(|(names, _)| names.iter().any(|name| name == "k"))
        .expect("`SUBJECT` is a callable");
    assert_eq!(*listed, some(&["Elt", "Key"]));
}

/// A module parameter is an unknown wherever a fact reads its content, so a callable reading a
/// member of one its signature pins keys on it.
#[test]
fn a_callable_keys_on_a_module_parameter_its_units_read() {
    let source = format!(
        "{PRELUDE}EXPR #(TOUCH m :(Counter WITH {{Carrier = Number}})) -> Any = #(\n  \
         LET z = m.zero\n  z\n)\n"
    );
    assert_eq!(keyed(&source, &["z", "m"]), some(&["m"]));
}

/// The keys fold reads every unit but a callable's entry, so a name only the parameters' own
/// declared types read keys nothing: `PAIR`'s body reads neither `First` nor `Second`.
#[test]
fn a_name_read_only_at_its_callable_s_entry_keys_nothing() {
    let source =
        "EXPR FOR ALL #[First Second] #(PAIR x :First WITH y :Second) -> Str = #(\"apart\")\n";
    assert_eq!(keyed(source, &["y", "x", "Second", "First"]), None);
}

thread_local! {
    /// Each use a frame found selected in its facts, by its body's segment and its site.
    static CHOSEN: std::cell::RefCell<Vec<(Segment, Site)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Note a use a frame found selected.
fn chose(segment: Segment, site: Site) {
    CHOSEN.with(|chosen| chosen.borrow_mut().push((segment, site)));
}

/// The site of `WRAP`'s `PICK` use in the program [`ran`] runs over `source`, and whether the load
/// selected it.
fn wraps_pick(source: &str) -> (Segment, Site, bool) {
    loaded(&program(2, source), |program| {
        let wrap = quantified(program);
        let Some(ExpressionPart::Expression(node)) = wrap.rhs(slot(program, wrap, "p")) else {
            panic!("`p` is bound to a call");
        };
        let site = wrap.site_of_node(node.reference());
        let narrowing = program.facts().use_statics(wrap, site).narrowing;
        (
            wrap.segment(),
            site,
            matches!(narrowing, Narrowing::Selected(_)),
        )
    })
}

#[test]
fn a_frame_under_a_settlement_reads_its_settled_facts() {
    // `PICK x` waits on `Elt` at the load, which leaves both overloads; a frame under the
    // settlement at `Elt := Number` finds the one at `Number` selected.
    let source = format!("{PICK}{}PRINT (WRAP 1)\n", WRAPS[0]);
    let (segment, site, selected) = wraps_pick(&source);
    assert!(!selected, "the load selects nothing at `PICK x`");
    CHOSEN.with(|chosen| chosen.borrow_mut().clear());
    crate::dispatch::evaluate::CHOSE.with(|hook| hook.set(Some(chose)));
    let ran = ran(&source);
    crate::dispatch::evaluate::CHOSE.with(|hook| hook.set(None));
    assert_eq!((ran.printed.as_str(), ran.tiers.settled), ("n", 1));
    let chosen = CHOSEN.with(|chosen| chosen.take());
    assert!(chosen.contains(&(segment, site)), "{chosen:?}");
}

thread_local! {
    /// The body whose units a test counts the walks' visits to, beside the count.
    static VISITS: std::cell::Cell<(Option<Segment>, usize)> = const { std::cell::Cell::new((None, 0)) };
}

/// Count a walk's visit to `unit`, where it is of the body counted.
fn visit(unit: crate::scope::UnitId) {
    VISITS.with(|visits| {
        let (counted, count) = visits.get();
        if counted == Some(unit.segment) {
            visits.set((counted, count + 1));
        }
    });
}

#[test]
fn a_fact_is_settled_once_per_settlement() {
    // Two frames under one key: the second finds the settlement the first's miss built, so the
    // units of `WRAP` its binding wakes are walked once past the load's walk.
    let visits = |calls: &str| {
        let source = format!("{PICK}{}{calls}", WRAPS[0]);
        let (wrap, _, _) = wraps_pick(&source);
        VISITS.with(|visits| visits.set((Some(wrap), 0)));
        let ran = crate::dispatch::walk::visiting(visit, || ran(&source));
        (ran.tiers.settled, VISITS.with(|visits| visits.get().1))
    };
    let (settled, walked) = visits("PRINT (WRAP 1)\n");
    let (_, loaded) = visits("LET unused = 1\n");
    assert_eq!(settled, 1);
    assert!(
        walked > loaded,
        "the miss walks `WRAP`'s body past the load"
    );
    assert_eq!(visits("PRINT (WRAP 1)\nPRINT (WRAP 2)\n"), (1, walked));
}

/// Two overloads of `PICK` at `Number`, which a call at `Number` cannot rank, and one at `Str`.
const TWINS: &str = "EXPR #(PICK x :Number) -> Str = #(\"a\")\n\
                     EXPR #(PICK x :Number) -> Str = #(\"b\")\n\
                     EXPR #(PICK x :Str) -> Number = #(1)\n";

/// `WRAP` over [`TWINS`]: at the load `PICK x` waits on `Elt`.
const TWIN_WRAP: &str = "EXPR FOR ALL #[Elt] #(WRAP x :Elt) -> Any = #((LET p = (PICK x)) (p))\n";

/// A refusal's message, its location dropped.
fn unlocated(message: &str) -> &str {
    message.split_once(": ").map_or(message, |(_, rest)| rest)
}

/// The refusal the load words for [`TWINS`] with `WRAP`'s `x` declared at `Number`.
fn declared_refusal() -> String {
    let declared = format!(
        "{TWINS}EXPR #(WRAP x :Number) -> Any = #((LET p = (PICK x)) (p))\nPRINT (WRAP 1)\n"
    );
    let refused = super::run(&declared);
    let refused = refused
        .strip_prefix("load: ")
        .expect("the declared program refuses");
    unlocated(refused).to_string()
}

/// The fault a run printed, its location dropped.
fn fault(printed: &str) -> &str {
    unlocated(printed.strip_prefix("error: ").expect("the run faults"))
}

#[test]
fn a_refusal_a_settlement_reaches_faults_the_asker_worded_as_the_load_words_it() {
    // `Elt := Number` makes `PICK x` certainly ambiguous: a load with `x` declared at `Number`
    // refuses, and a frame under the settlement faults with that refusal. The second field's ask
    // finds the refused settlement under the key and is born under it too.
    let ran = ran(&format!(
        "{TWINS}{TWIN_WRAP}PRINT {{a = (WRAP 1), b = (WRAP 2)}}\n"
    ));
    assert_eq!(fault(&ran.printed), declared_refusal());
    assert_eq!((ran.tiers.sought, ran.tiers.settled), (2, 1));
}

#[test]
fn a_settlements_facts_and_refusals_are_the_same_after_its_eviction() {
    // The fields' frames run one after another, `WRAP "s"`'s between the two at `Number`: past the
    // smallest budget, its miss evicts the refused settlement, and the next miss settles it again,
    // refusing alike.
    let source =
        format!("{TWINS}{TWIN_WRAP}PRINT {{a = (WRAP 1), b = (WRAP \"s\"), c = (WRAP 2)}}\n");
    let tight = ran_under(2, &source, SMALLEST);
    let roomy = ran(&source);
    assert_eq!(tight.printed, roomy.printed);
    assert_eq!(fault(&tight.printed), declared_refusal());
    assert_eq!((tight.tiers.settled, tight.tiers.evicted), (3, 2));
    assert_eq!((roomy.tiers.settled, roomy.tiers.evicted), (2, 0));
}
