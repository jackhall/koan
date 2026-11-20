//! The decision memo: a cell running a body over the bindings an earlier cell of its chain decided
//! reuses that decision, kept in the region the chain's results are built in, rather than walking.

use proptest::prelude::*;

use crate::program::activation_store::{DECIDED, KEPT, UNMEMOIZED};
use crate::values::BuildBudget;

use super::caching::{Ran, ran_under};

/// `MAKE`, a recursive functor the load never unfolds, whose result is content the run produced;
/// `PICK`, whose overloads tell a carrier of numbers from one of strings; and `NEXT`, which makes
/// `ints` or `words` over the chain by the type of what it is handed.
pub(super) const MAKE: &str = "EXPR #(MAKE m :Counter link :Link) -> Counter = #(MAKE m (ATTR link next))\n\
                    EXPR #(MAKE m :Counter end :Null) -> Counter = #(m)\n\
                    EXPR #(PICK x :Number) -> Str = #(\"n\")\n\
                    EXPR #(PICK x :Str) -> Number = #(1)\n\
                    EXPR #(NEXT x :Number) -> Counter = #(MAKE ints chain)\n\
                    EXPR #(NEXT x :Str) -> Counter = #(MAKE words chain)\n\
                    EXPR #(FLIP x :Number) -> Any = #(\"s\")\n\
                    EXPR #(FLIP x :Str) -> Any = #(1)\n";

/// Run `source` over a chain of `depth` links, counting the binding events that walked.
fn ran(depth: u32, source: &str) -> (Ran, usize) {
    DECIDED.with(|decided| decided.set(0));
    KEPT.with(|kept| kept.set(0));
    let ran = ran_under(depth, &format!("{MAKE}{source}"), BuildBudget::default());
    (ran, DECIDED.with(std::cell::Cell::get))
}

/// `PEEK`, whose overloads tell a string from anything else.
const PEEK: &str = "EXPR #(PEEK x :Any) -> Str = #(\"a\")\n\
                    EXPR #(PEEK x :Str) -> Number = #(1)\n";

#[test]
fn a_fresh_loop_whose_carrier_repeats_decides_once() {
    // `PAIR` hands `LOOP` a record whose type the run composed, so each hop's entry binds `Elt` to
    // the same type without keying a settlement: the first hop decides, and every later one reads
    // its decision from the memo in the loop's home.
    let source = format!(
        "{PEEK}EXPR FOR ALL #[Elt] #(LOOP x :Elt link :Link) -> Number = \
         #((PRINT (PEEK x)) (LOOP x (ATTR link next)))\n\
         EXPR FOR ALL #[Elt] #(LOOP x :Elt end :Null) -> Number = #(7)\n\
         EXPR FOR ALL #[Item] #(PAIR y :Item) -> Number = #(LOOP {{a = y, b = y}} chain)\n\
         PRINT (PAIR 1)\n"
    );
    for depth in [1, 4, 16] {
        let (ran, decided) = ran(depth, &source);
        assert_eq!(ran.printed, format!("{}7", "a\n".repeat(depth as usize)));
        assert_eq!(decided, 1, "the first hop decides, at depth {depth}");
    }
}

/// A `Fresh` loop whose entry binds `Elt` to the same type at every hop, and whose `r` alternates
/// between `ints` and `words`.
fn alternating() -> String {
    format!(
        "{PEEK}EXPR FOR ALL #[Elt] #(LOOP e :Elt x :Any link :Link) -> Number = \
         #((LET r = (NEXT x)) (PRINT (PICK r.zero)) (PRINT (PEEK e)) \
         (LOOP e (FLIP x) (ATTR link next)))\n\
         EXPR FOR ALL #[Elt] #(LOOP e :Elt x :Any end :Null) -> Number = #(7)\n\
         EXPR FOR ALL #[Item] #(PAIR y :Item) -> Number = #(LOOP {{a = y}} 1 chain)\n\
         PRINT (PAIR 1)\n"
    )
}

/// What [`alternating`] prints over `depth` links.
fn alternated(depth: u32) -> String {
    let hops = (0..depth).map(|hop| if hop % 2 == 0 { "n\na\n" } else { "1\na\n" });
    format!("{}7", hops.collect::<String>())
}

#[test]
fn a_statement_binding_that_alternates_decides_each_binding_once() {
    // Each hop reads its entry's decision, and `r`'s statement alternates between two bindings,
    // which the entry holds side by side under it: each is decided once.
    for depth in [2, 8] {
        let (ran, decided) = ran(depth, &alternating());
        assert_eq!(ran.printed, alternated(depth));
        assert_eq!(decided, 3, "the entry once, `r` once per binding");
    }
}

/// A `Fresh` loop whose entry binds `Elt` to a deeper record type at every hop, running `held`
/// first: every hop misses by identity.
fn deepening_with(held: &str) -> String {
    format!(
        "{PEEK}EXPR FOR ALL #[Elt] #(LOOP x :Elt link :Link) -> Number = \
         #({held}(PRINT (PEEK x)) (LOOP {{a = x}} (ATTR link next)))\n\
         EXPR FOR ALL #[Elt] #(LOOP x :Elt end :Null) -> Number = #(7)\n\
         EXPR FOR ALL #[Item] #(PAIR y :Item) -> Number = #(LOOP {{a = y}} chain)\n\
         PRINT (PAIR 1)\n"
    )
}

/// A statement over `Elt` whose unit holds under every binding: a registration whose `FOR ALL`
/// bound names `Elt` is left to the run.
const HELD: &str = "(EXPR FOR ALL #{Tee: Elt} #(GEN t :Tee) -> Tee = #(t)) ";

/// [`deepening_with`] running nothing first.
pub(super) fn deepening() -> String {
    deepening_with("")
}

#[test]
fn a_fresh_loop_missing_every_hop_keeps_its_memos_budget() {
    // Each hop's entry binds a type no hop bound before, so, walked closed, it misses: the loop's
    // home keeps a decision at each hop until its budget is spent, then only decides. Past the
    // budget a hop lays only its own record type, one node, in the home.
    crate::program::resume::UNWITNESSED.with(|unwitnessed| unwitnessed.set(true));
    let runs = [40, 80].map(|depth| {
        let (ran, decided) = ran(depth, &deepening());
        assert_eq!(ran.printed, format!("{}7", "a\n".repeat(depth as usize)));
        assert_eq!(decided, depth as usize, "every hop misses");
        (ran.tiers.region, KEPT.with(std::cell::Cell::get))
    });
    crate::program::resume::UNWITNESSED.with(|unwitnessed| unwitnessed.set(false));
    assert_eq!(
        (runs[0].1, runs[1].1),
        (32, 32),
        "the budget's worth of decisions"
    );
    assert!(
        runs[1].0 - runs[0].0 <= 40,
        "a node per hop past the budget: {runs:?}"
    );
}

/// Whether a witnessed loop over `source` keeps `kept` decisions in its home at depths 40 and 80,
/// and lays a node per hop past the budget.
fn keeps_its_memos_budget(source: &str, kept: usize) {
    let runs = [40, 80].map(|depth| {
        let (ran, _) = ran(depth, source);
        assert_eq!(ran.printed, format!("{}7", "a\n".repeat(depth as usize)));
        (ran.tiers.region, KEPT.with(std::cell::Cell::get))
    });
    assert_eq!((runs[0].1, runs[1].1), (kept, kept), "decisions kept");
    assert!(
        runs[1].0 - runs[0].0 <= 40,
        "a node per hop past the budget: {runs:?}"
    );
}

#[test]
fn a_witnessed_loop_keeps_its_memos_budget() {
    // Where nothing holds, a hop joins its first decision walking nothing and keeps nothing: the
    // home keeps that decision alone, and each hop lays a node.
    keeps_its_memos_budget(&deepening(), 1);
}

#[test]
fn a_witnessed_loop_over_a_held_unit_keeps_its_memos_budget() {
    // Witnessed, a loop whose hops bind novel types and whose statement holds keeps a binding
    // layer per hop in its home, until the budget is spent; past it a hop lays a node in the home.
    keeps_its_memos_budget(&deepening_with(HELD), 32);
}

#[test]
fn a_loop_past_its_memos_budget_witnesses_no_more() {
    // Five `r`s cycle past the fanout, so each lap drops and witnesses again: a witnessed layer
    // per hop until the home's budget is spent. Past it the memo keeps nothing, so a miss walks
    // closed rather than witness a layer no later hop could read.
    let printed = |depth: u32| format!("{}7", "n\n".repeat(depth as usize));
    let runs = [40, 80].map(|depth| counted(depth, &cycling(5), &printed(depth)));
    let [shallow, deep] = &runs;
    assert_eq!(shallow.witness_walks, deep.witness_walks, "{runs:?}");
    assert!(
        deep.decided > shallow.decided,
        "past the budget a miss walks closed: {runs:?}"
    );
}

/// What a run of `source` over `depth` links counted: binding events that walked, binding layers
/// laid, witnessing walks, and units the closing walks and the witnessing walks re-ran.
#[derive(Debug, PartialEq)]
struct Witnessed {
    decided: usize,
    laid: usize,
    witness_walks: usize,
    reruns: usize,
    witnessed: usize,
}

fn witnessed(depth: u32, source: &str) -> Witnessed {
    counted(depth, source, &format!("{}7", "a\n".repeat(depth as usize)))
}

/// [`witnessed`], printing `printed`.
fn counted(depth: u32, source: &str, printed: &str) -> Witnessed {
    use crate::dispatch::walk::{RERUNS, WITNESSED};
    use crate::program::resume::{LAYERS_LAID, WITNESS_WALKS};
    let sum = |counts: &'static std::thread::LocalKey<std::cell::Cell<[usize; _]>>| {
        counts.with(|counts| counts.get().iter().sum::<usize>())
    };
    LAYERS_LAID.with(|laid| laid.set(0));
    WITNESS_WALKS.with(|walks| walks.set(0));
    RERUNS.with(|reruns| reruns.set(Default::default()));
    WITNESSED.with(|reruns| reruns.set(Default::default()));
    let (ran, decided) = ran(depth, source);
    assert_eq!(ran.printed, printed);
    Witnessed {
        decided,
        laid: LAYERS_LAID.with(std::cell::Cell::get),
        witness_walks: WITNESS_WALKS.with(std::cell::Cell::get),
        reruns: sum(&RERUNS),
        witnessed: sum(&WITNESSED),
    }
}

#[test]
fn a_loop_binding_novel_types_that_keep_every_question_decides_once() {
    // Each hop's entry binds `Elt` to a record type no hop bound before, so it misses by identity;
    // but every question its first hop's walk answered, every later binding keeps, and nothing
    // holds: a hop past the first walks nothing and lays only its table of closed types.
    let [shallow, deep] = [4, 16].map(|depth| witnessed(depth, &deepening()));
    assert_eq!(shallow.decided, deep.decided, "{shallow:?} {deep:?}");
    assert_eq!(deep.laid - shallow.laid, 12, "one binding layer per hop");
    assert_eq!(
        (shallow.reruns, shallow.witnessed),
        (deep.reruns, deep.witnessed)
    );
    assert_eq!(
        shallow.witness_walks, deep.witness_walks,
        "the loop witnesses once"
    );
}

#[test]
fn a_deepening_loop_witnesses_once_and_re_runs_only_what_holds() {
    // A registration whose `FOR ALL` bound names `Elt` is left to the run, so its unit holds: every
    // hop re-runs it in a closing walk over the first hop's witnessed layer, and witnesses nothing.
    let source = deepening_with(HELD);
    let runs = [4, 8, 16].map(|depth| witnessed(depth, &source));
    let [a, b, c] = &runs;
    assert_eq!(a.witness_walks, c.witness_walks, "{runs:?}");
    assert_eq!(
        a.witnessed, c.witnessed,
        "witnessing walks past the first hop: {runs:?}"
    );
    assert_eq!(
        c.decided - b.decided,
        2 * (b.decided - a.decided),
        "{runs:?}"
    );
    assert!(
        b.decided > a.decided,
        "a held unit walks each hop: {runs:?}"
    );
    assert_eq!(c.reruns - b.reruns, 2 * (b.reruns - a.reruns), "{runs:?}");
}

/// [`alternating`] whose entry binds `Elt` to a deeper record type at every hop.
pub(super) fn alternating_deepening() -> String {
    alternating().replace("(LOOP e (FLIP x)", "(LOOP {a = e} (FLIP x)")
}

#[test]
fn a_statement_under_a_deepening_entry_witnesses_once_per_binding_class() {
    // Each hop's entry binds `Elt` to a type no hop bound before, and `r` alternates between two
    // modules: `r`'s statement witnesses `Elt` beside its own binding, so its witnessed layer
    // serves every entry binding keeping its questions, and it witnesses once per module, not
    // once per hop. Past the second hop no event walks; each lays its two tables of closed types.
    let runs = [4, 16].map(|depth| counted(depth, &alternating_deepening(), &alternated(depth)));
    let [shallow, deep] = &runs;
    assert_eq!(shallow.decided, deep.decided, "{runs:?}");
    assert_eq!(shallow.witness_walks, deep.witness_walks, "{runs:?}");
    assert_eq!(
        deep.laid - shallow.laid,
        2 * 12,
        "two layers per hop: {runs:?}"
    );
    assert_eq!(
        (shallow.reruns, shallow.witnessed),
        (deep.reruns, deep.witnessed),
        "{runs:?}"
    );
}

/// [`alternating_deepening`] whose entry holds [`HELD`]'s registration, and whose `m`, after
/// `r`'s statement, reads `r` and, where `reads`, a call of it.
fn reading_held(reads: bool) -> String {
    let m = match reads {
        true => "(LET m = {p = (GEN e), q = r.zero})",
        false => "(LET m = {q = r.zero})",
    };
    alternating_deepening().replace(
        "#((LET r = (NEXT x))",
        &format!("#({HELD}(LET r = (NEXT x)) {m}"),
    )
}

#[test]
fn a_statement_reading_a_held_unit_re_runs_each_hop() {
    // The registration holds, so every hop's entry walks. `m` reads `r` and a call of it: its
    // record over `r`'s witnessed layer names the registration's held marks, so `r`'s statement
    // walks each hop too.
    let source = reading_held(true);
    let runs = [4, 8].map(|depth| counted(depth, &source, &alternated(depth)));
    let [shallow, deep] = &runs;
    assert_eq!(deep.decided - shallow.decided, 2 * 4, "{runs:?}");
    assert_eq!(shallow.witness_walks, deep.witness_walks, "{runs:?}");
}

#[test]
#[ignore = "under a held registration, `r`'s binding event walks each hop though `m` reads no call"]
fn a_statement_apart_from_a_held_unit_is_absorbed() {
    // The registration holds, so every hop's entry walks. Apart from a call of it, `r`'s statement
    // reads over its witnessed layer and walks nothing.
    let source = reading_held(false);
    let runs = [4, 8].map(|depth| counted(depth, &source, &alternated(depth)));
    let [shallow, deep] = &runs;
    assert_eq!(deep.decided - shallow.decided, 4, "{runs:?}");
    assert_eq!(shallow.witness_walks, deep.witness_walks, "{runs:?}");
}

/// A `Fresh` loop whose entry binds `Elt` to the same type at every hop, and whose `r` cycles
/// through `cycle` modules, one per hop: each hop's `x` is of the next of `cycle` types, which
/// `ROT` steps along and `NEXT` makes a module of.
fn cycling(cycle: usize) -> String {
    let types = ["Number", "Str", "Null", "{a :Number}", "{b :Number}"];
    let values = ["1", "\"s\"", "null", "{a = 1}", "{b = 1}"];
    let mut source = String::new();
    for at in 0..cycle {
        source += &format!(
            "MODULE m{at} = ((LET zero = {at}) (LET succ = (FN :{{x :Number}} -> Number = #(x))))\n\
             EXPR #(ROT x :{}) -> Any = #({})\n\
             EXPR #(CYCLE x :{}) -> Counter = #(MAKE m{at} chain)\n",
            types[at],
            values[(at + 1) % cycle],
            types[at],
        );
    }
    format!(
        "{source}EXPR FOR ALL #[Elt] #(LOOP e :Elt x :Any link :Link) -> Number = \
         #((LET r = (CYCLE x)) (PRINT (PICK r.zero)) (LOOP e (ROT x) (ATTR link next)))\n\
         EXPR FOR ALL #[Elt] #(LOOP e :Elt x :Any end :Null) -> Number = #(7)\n\
         EXPR FOR ALL #[Item] #(PAIR y :Item) -> Number = #(LOOP {{a = y}} 1 chain)\n\
         PRINT (PAIR 1)\n"
    )
}

#[test]
fn an_event_holds_at_most_four_decisions_the_earliest_dropped() {
    let printed = |depth: u32| format!("{}7", "n\n".repeat(depth as usize));
    // Four bindings of `r` fit under the entry's decision: each is decided once.
    let (ran4, decided) = ran(8, &cycling(4));
    assert_eq!(ran4.printed, printed(8));
    assert_eq!(decided, 1 + 4, "the entry once, each of four `r`s once");
    // A fifth drops the earliest, which the next lap then misses, and so on round.
    for depth in [5, 10] {
        let (ran, decided) = ran(depth, &cycling(5));
        assert_eq!(ran.printed, printed(depth));
        assert_eq!(
            decided,
            1 + depth as usize,
            "every `r` misses, at depth {depth}"
        );
    }
}

/// `PING` and `PONG`, which call each other down the chain over a record whose type the run
/// composed, through `call`: a tail call where it is the body's last statement, a nested one where
/// its value is bound first. `VIA` hands `PAIR` its argument at `Any`, so the load lays no layer for
/// `PAIR`, which would compose the record at the load.
fn ping_pong(call: &str) -> String {
    let body = |other: &str| call.replace("OTHER", other);
    let (ping, pong) = (body("PONG"), body("PING"));
    format!(
        "{PEEK}EXPR FOR ALL #[Elt] #(PING x :Elt link :Link) -> Number = #({ping})\n\
         EXPR FOR ALL #[Elt] #(PING x :Elt end :Null) -> Number = #(7)\n\
         EXPR FOR ALL #[Elt] #(PONG x :Elt link :Link) -> Number = #({pong})\n\
         EXPR FOR ALL #[Elt] #(PONG x :Elt end :Null) -> Number = #(7)\n\
         EXPR FOR ALL #[Item] #(PAIR y :Item) -> Number = #(PING {{a = y}} chain)\n\
         EXPR #(VIA y :Any) -> Any = #(PAIR y)\n\
         PRINT (VIA 1)\n"
    )
}

#[test]
fn a_mutual_tail_loop_decides_each_body_once() {
    let source = ping_pong("(PRINT (PEEK x)) (OTHER x (ATTR link next))");
    let (ran, decided) = ran(8, &source);
    assert_eq!(ran.printed, format!("{}7", "a\n".repeat(8)));
    assert_eq!(decided, 2, "`PING` once and `PONG` once");
}

#[test]
fn a_nested_mutual_recursion_decides_each_body_once() {
    // Each frame is the home of the one it calls, whose memo it seeds from its own: every level
    // finds what the first `PING` and the first `PONG` decided.
    let source = ping_pong("(LET n = (OTHER x (ATTR link next))) (PRINT (PEEK x)) (n)");
    let (ran, decided) = ran(8, &source);
    assert_eq!(ran.printed, format!("{}7", "a\n".repeat(8)));
    assert_eq!(decided, 2, "`PING` once and `PONG` once");
}

/// `BOTH`, whose overloads tell a carrier of numbers from one of strings by its first operand.
const BOTH: &str = "EXPR #(BOTH x :Number AND y :Any) -> Str = #(\"nn\")\n\
                    EXPR #(BOTH x :Str AND y :Any) -> Number = #(1)\n";

#[test]
fn a_body_under_another_settlement_misses_the_memo() {
    // `RUNS` keys a settlement on `k`, and on `j`, which it passes on, handed `ints` at `Any` by
    // `VIA`, so the load lays no layer for it and it settles at run: the chain walks under `ints`,
    // then its end, keyed on the slots it passes on, hops to a walk under `words`. `r`'s statement
    // binds the same content at every hop, and decides once under each settlement; the end that
    // hops between them decides its entry in its own settlement.
    let source = format!(
        "{BOTH}EXPR #(RUNS k :Counter j :Counter link :Link stop :Any) -> Number = \
         #((PRINT (PICK k.zero)) (LET r = (MAKE ints chain)) (PRINT (BOTH r.zero AND k.zero)) \
         (RUNS k j (ATTR link next) stop))\n\
         EXPR #(RUNS k :Counter j :Counter end :Null stop :Link) -> Number = \
         #(RUNS j k stop null)\n\
         EXPR #(RUNS k :Counter j :Counter end :Null stop :Null) -> Number = #(7)\n\
         EXPR #(VIA k :Any) -> Any = #(RUNS k words chain chain)\n\
         PRINT (VIA ints)\n"
    );
    let (ran, decided) = ran(4, &source);
    assert_eq!(
        ran.printed,
        format!("{}{}7", "n\nnn\n".repeat(4), "1\nnn\n".repeat(4))
    );
    // The load lays `MAKE ints chain`'s layer; its recursion into the end overload settles.
    assert_eq!(
        (ran.tiers.settled, ran.tiers.laid > 0),
        (4, true),
        "`MAKE`'s end, `RUNS`'s two, and the end's own"
    );
    assert_eq!(decided, 2, "`r` once under each settlement");
}

#[test]
fn a_frame_from_another_enclosers_activation_misses_on_its_captures() {
    // Each hop of `STEP` binds `r` to `ints` or `words` in turn and tails into a new `g`, whose entry
    // binds `p` the same each hop and reads `r` through its closure: `g` decides once for each
    // `r` it captured, and reuses the decision under the same one.
    let source = format!(
        "{BOTH}EXPR #(STEP x :Any n :Number link :Link) -> Number = #(\n  \
         LET r = (NEXT x)\n  \
         MODULE h OVER #[n] = ((LET zero = n) (LET succ = (FN :{{x :Number}} -> Number = #(x))))\n  \
         LET g = (FN :{{p :Counter y :Any l :Any}} -> Number = \
         #((PRINT (BOTH r.zero AND p.zero)) (STEP y 1 l)))\n  \
         (g {{p = h, y = (FLIP x), l = (ATTR link next)}})\n)\n\
         EXPR #(STEP x :Any n :Number end :Null) -> Number = #(7)\n\
         PRINT (STEP 1 1 chain)\n"
    );
    let (ran, decided) = ran(8, &source);
    let printed = (0..8)
        .map(|hop| if hop % 2 == 0 { "nn\n" } else { "1\n" })
        .collect::<String>();
    assert_eq!(ran.printed, format!("{printed}7"));
    assert_eq!(decided, 2 + 2, "`r` once per binding, and `g` once per `r`");
}

/// `MK`, which builds a record of `x` beside `k`, a number or a string, through a dispatch on
/// `k`'s type, so the member's type follows the overload.
const MK: &str = "EXPR FOR ALL #[Inner] #(MK x :Inner k :Number) -> Any = #({a = x, b = k})\n\
                  EXPR FOR ALL #[Inner] #(MK x :Inner k :Str) -> Any = #({a = x, b = k})\n";

/// A chain of nodes, the `at`-th holding `hops[at]`: whether its statement binds `r` to `ints`,
/// whether its block surfaces `ints`, else `words`, and whether its `k` is a number, else a
/// string; then `WALK`, which binds both at each hop and decides over them, its block reading `e`,
/// and tails through `GEE`, a callee born in its body. Its entry binds the same type throughout,
/// or, where it `deepens`, `MK`'s record of the last beside the node's `k`.
fn walked(hops: &[(bool, bool, bool)], deepens: bool) -> String {
    let tag = |ints: bool| if ints { "1" } else { "\"s\"" };
    let next = if deepens { "(MK e (ATTR node k))" } else { "e" };
    let mut source = format!(
        "{PEEK}{MK}EXPR #(TELL r :{{d :{{b :Number}}}}) -> Str = #(\"num\")\n\
         EXPR #(TELL r :Any) -> Str = #(\"other\")\n\
         NEWTYPE Node = :{{next :Any, s :Any, b :Any, k :Any}}\n\
         EXPR FOR ALL #[Elt] #(WALK e :Elt node :Node) -> Number = #(\n  \
         LET r = (NEXT (ATTR node s))\n  PRINT (PICK r.zero)\n  \
         LET q = (NEXT (ATTR node b))\n  \
         PRINT (USING (q :! Counter) SCOPE (TELL {{c = zero, d = e}}))\n  \
         PRINT (PEEK e)\n  \
         EXPR FOR ALL #[Tee] #(GEE t :Tee n :Any) -> Number = \
         #((PRINT (TELL {{c = t, d = e}})) (WALK {next} n))\n  \
         GEE {{z = q.zero}} (ATTR node next)\n)\n\
         EXPR FOR ALL #[Elt] #(WALK e :Elt end :Null) -> Number = #(7)\n\
         EXPR FOR ALL #[Item] #(PAIR y :Item) -> Number = #(WALK {{a = y}} node0)\n\
         EXPR #(VIA y :Any) -> Any = #(PAIR y)\n\
         LET node{} = null\n",
        hops.len()
    );
    for (at, (statement, block, k)) in hops.iter().enumerate().rev() {
        source.push_str(&format!(
            "LET node{at} = (Node {{next = node{}, s = {}, b = {}, k = {}}})\n",
            at + 1,
            tag(*statement),
            tag(*block),
            tag(*k)
        ));
    }
    source.push_str("PRINT (VIA 1)\n");
    source
}

#[test]
fn the_walked_program_prints_each_hops_answers() {
    // The law's generator at four hops, deepening, each `MK` turning `b`'s type.
    let hops = [
        (false, false, true),
        (true, true, false),
        (false, true, true),
        (true, false, false),
    ];
    let (ran, _) = ran(0, &walked(&hops, true));
    assert_eq!(
        ran.printed,
        "1\nother\na\nother\nn\nnum\na\nnum\n1\nother\na\nother\nn\nnum\na\nnum\n7"
    );
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: crate::tests::case_share(1, 4),
        ..ProptestConfig::default()
    })]

    /// A memoized activation is one decided from scratch: a walk whose hops bind its statement and
    /// its block from a small pool of carriers prints what it prints with every event walked, and
    /// walks no more often.
    #[test]
    fn a_memoized_activation_equals_one_decided_from_scratch(
        hops in prop::collection::vec((any::<bool>(), any::<bool>(), any::<bool>()), 1..8),
        deepens in any::<bool>(),
    ) {
        let source = walked(&hops, deepens);
        let (memoized, decided) = ran(0, &source);
        UNMEMOIZED.with(|off| off.set(true));
        let (scratch, walks) = ran(0, &source);
        UNMEMOIZED.with(|off| off.set(false));
        prop_assert_eq!(&memoized.printed, &scratch.printed, "in\n{}", source);
        prop_assert!(!memoized.printed.contains("error"), "{}", memoized.printed);
        prop_assert!(decided <= walks, "{} walks memoized, {} from scratch", decided, walks);
    }
}

/// `SHAPE`, which tells a record of a number at `a` from anything else; `DUO`, whose overloads
/// tell a second operand of numbers from one of strings; and `SWAP`, which hands a record of a
/// number at `a` a record of a string there, and anything else a record of a number.
const SHAPE_DUO_SWAP: &str = "EXPR #(SHAPE x :{a :Number}) -> Number = #(1)\n\
                              EXPR #(SHAPE x :Any) -> Str = #(\"a\")\n\
                              EXPR #(DUO x :Number y :Number) -> Str = #(\"nn\")\n\
                              EXPR #(DUO x :Number y :Str) -> Str = #(\"ns\")\n\
                              EXPR #(SWAP x :{a :Number}) -> Any = #({a = \"s\"})\n\
                              EXPR #(SWAP x :Any) -> Any = #({a = 1})\n";

#[test]
fn a_seam_reading_a_local_whose_answer_the_entry_binding_breaks_re_runs() {
    // `SWAP` binds `Elt` to `{a :Number}` and `{a :Str}` in turn, so `a`'s overload of `SHAPE`
    // alternates. `r`'s seam witnesses `Elt` beside `r`; its `DUO` record reaches `SHAPE`'s answer
    // on `Elt` through its read of `a`, so the seam's admission holds that question, and a hop whose
    // entry binding breaks it re-runs `DUO`.
    let source = format!(
        "{SHAPE_DUO_SWAP}EXPR FOR ALL #[Elt] #(LOOP x :Elt link :Link) -> Number = #(\n  \
         LET a = (SHAPE x)\n  LET r = (NEXT 1)\n  PRINT (DUO r.zero a)\n  \
         LOOP (SWAP x) (ATTR link next)\n)\n\
         EXPR FOR ALL #[Elt] #(LOOP x :Elt end :Null) -> Number = #(7)\n\
         EXPR FOR ALL #[Item] #(PAIR y :Item) -> Number = #(LOOP {{a = y}} chain)\n\
         PRINT (PAIR 1)\n"
    );
    let (ran, _) = ran(4, &source);
    assert_eq!(ran.printed, "nn\nns\nnn\nns\n7");
}

#[test]
fn a_frame_built_record_over_a_name_bound_to_an_any_member_checks_in_its_home() {
    // The tail call's record is built in the chain's home, its static type closed through the
    // frame's chain: `Elt` binds `b :Any` from `WIDEN`'s static type, though the record carries
    // `b :Number`. A debug build's check of the home build skips static ends the home's chain does
    // not resolve.
    let source = "EXPR #(WIDEN x :Any) -> Any = #(x)\n\
                  EXPR FOR ALL #[Elt] #(LOOP x :Elt link :Link) -> Number = #(\n  \
                  PRINT 0\n  LOOP {a = x, b = (WIDEN 1)} (ATTR link next)\n)\n\
                  EXPR FOR ALL #[Elt] #(LOOP x :Elt end :Null) -> Number = #(7)\n\
                  EXPR FOR ALL #[Item] #(PAIR y :Item) -> Number = \
                  #(LOOP {a = y, b = (WIDEN 1)} chain)\n\
                  PRINT (PAIR 1)\n";
    let (ran, _) = ran(4, source);
    assert_eq!(ran.printed, "0\n0\n0\n0\n7");
}

thread_local! {
    /// Each binding event's site, beside whether it walked, while a test counts them.
    static EVENTS: std::cell::RefCell<Vec<(crate::scope::EventSite, bool)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

fn noted(site: crate::scope::EventSite, walked: bool) {
    EVENTS.with(|events| events.borrow_mut().push((site, walked)));
}

/// The segment of the first shape of `kind` in [`MAKE`] and `source` over `depth` links with a
/// slot named `name`.
fn segment_of(
    depth: u32,
    source: &str,
    kind: crate::scope::ShapeKind,
    name: &str,
) -> crate::scope::Segment {
    use crate::scope::Slot;
    let program = super::caching::program(depth, &format!("{MAKE}{source}"));
    super::statics::loaded(&program, |program| {
        let mut shapes = Vec::new();
        super::caching::every_shape(program.shape(), &mut shapes);
        (shapes.into_iter())
            .find(|shape| {
                shape.kind() == kind
                    && (0..shape.slots()).any(|at| {
                        let slot = shape.slot_name(Slot(at as u32)).symbol();
                        program.symbols().display(slot).to_string() == name
                    })
            })
            .unwrap_or_else(|| panic!("a {kind:?} with `{name}` is written"))
            .segment()
    })
}

/// How often the events of each of `segments` walked over a run of `source` at `depth`,
/// printing `printed`.
fn walks_at<const N: usize>(
    depth: u32,
    source: &str,
    printed: &str,
    segments: [crate::scope::Segment; N],
) -> [usize; N] {
    use crate::program::DECIDING;
    EVENTS.with(|events| events.borrow_mut().clear());
    DECIDING.with(|hook| hook.set(Some(noted)));
    let (ran, _) = ran(depth, source);
    DECIDING.with(|hook| hook.set(None));
    assert_eq!(ran.printed, printed);
    let events = EVENTS.with(|events| events.take());
    segments.map(|segment| {
        (events.iter())
            .filter(|(site, walked)| *walked && site.segment() == segment)
            .count()
    })
}

/// A loop whose body holds a block, under `empty`, reading `x` beside its own `r`, and a callee
/// born in it, called in its tail chain, reading `x` beside its own `t`, each through `tell`. The
/// loop's entry binds `Elt`, and `k` where `flips`; the next hop's entry binds `{a = x}`, or, where
/// it `flips`, `MK`'s record of `x` beside a flipped `k`.
fn under_enclosers(tell: &str, flips: bool) -> String {
    let (k, next, first) = match flips {
        true => (" k :Any", "(MK x (FLIP k)) (FLIP k)", "(MK y 1) 1"),
        false => ("", "{a = x}", "{a = y}"),
    };
    format!(
        "EXPR FOR ALL #[Elt] #(LOOP x :Elt{k} link :Link) -> Number = #(\n  \
         LET q = (NEXT 1)\n  \
         PRINT (USING empty SCOPE (\n    LET r = (NEXT 1)\n    {tell} {{c = r.zero, e = x}}\n  ))\n  \
         EXPR FOR ALL #[Tee] #(GEE t :Tee l :Any) -> Number = \
         #((PRINT ({tell} {{c = t, e = x}})) (LOOP {next} l))\n  \
         GEE {{z = q.zero}} (ATTR link next)\n)\n\
         EXPR FOR ALL #[Elt] #(LOOP x :Elt{k} end :Null) -> Number = #(7)\n\
         EXPR FOR ALL #[Item] #(PAIR y :Item) -> Number = #(LOOP {first} chain)\n\
         EXPR #(VIA y :Any) -> Any = #(PAIR y)\n\
         PRINT (VIA 1)\n"
    )
}

/// The block's and the callee's segments in [`under_enclosers`]'s `source` over `depth` links.
fn cells(depth: u32, source: &str) -> [crate::scope::Segment; 2] {
    use crate::scope::ShapeKind;
    [
        segment_of(depth, source, ShapeKind::Block, "r"),
        segment_of(depth, source, ShapeKind::Callable, "t"),
    ]
}

#[test]
fn a_block_and_a_callee_under_a_deepening_encloser_decide_once() {
    // The loop's entry binds `Elt` to a novel type each hop, and every question the block's and
    // `GEE`'s events ask takes the same answer: each decides once, at the first hop.
    let source = format!("{PEEK}{}", under_enclosers("PEEK", false));
    let [shallow, deep] = [4, 16].map(|depth| {
        let printed = format!("{}7", "a\na\n".repeat(depth as usize));
        walks_at(depth, &source, &printed, cells(depth, &source))
    });
    assert!(shallow.iter().all(|walks| *walks >= 1), "{shallow:?}");
    assert_eq!(shallow, deep, "walks at depth 4 and 16");
}

/// `TELL`, whose overloads tell a record whose `e` holds a number at `b` from anything else.
const TELL: &str = "EXPR #(TELL r :{e :{b :Number}}) -> Str = #(\"num\")\n\
                       EXPR #(TELL r :Any) -> Str = #(\"other\")\n";

#[test]
fn a_block_and_a_callee_under_a_deepening_encloser_decide_once_per_answer() {
    // Each hop's `Elt` is novel, its `b` member a number and a string in turn: the loop's entry
    // joins its first decision at the second hop, so its spine repeats, while the block's and
    // `GEE`'s answers on `Elt` alternate. Each decides once per answer.
    let source = format!("{MK}{TELL}{}", under_enclosers("TELL", true));
    let [shallow, deep] = [4, 16].map(|depth| {
        let hop = "num\nnum\nother\nother\n";
        let printed = format!("{}7", hop.repeat(depth as usize / 2));
        walks_at(depth, &source, &printed, cells(depth, &source))
    });
    assert!(shallow.iter().all(|walks| *walks >= 2), "{shallow:?}");
    assert_eq!(shallow, deep, "walks at depth 4 and 16");
}

/// A nested (non-tail) loop whose entry binds `Elt` to a deeper record type at every level and
/// holds nothing, so after the first level it lays nothing; and whose block, run before the
/// nested call, registers `INNER` over a bound naming `Elt`, which the run registers, so it holds,
/// walks and lays a decision at every level, which each deeper level's memo is seeded with.
fn nested_deepening() -> String {
    format!(
        "{PEEK}EXPR FOR ALL #[Elt] #(LOOP x :Elt link :Link) -> Number = #(\n  \
         PRINT (USING empty SCOPE (\n    LET r = (NEXT 1)\n    \
         EXPR FOR ALL #{{Tee: Elt}} #(INNER y :Tee) -> Tee = #(y)\n    \
         PEEK {{c = r.zero, e = (INNER x)}}\n  ))\n  \
         LET n = (LOOP {{a = x}} (ATTR link next))\n  n\n)\n\
         EXPR FOR ALL #[Elt] #(LOOP x :Elt end :Null) -> Number = #(7)\n\
         EXPR FOR ALL #[Item] #(PAIR y :Item) -> Number = #(LOOP {{a = y}} chain)\n\
         EXPR #(VIA y :Any) -> Any = #(PAIR y)\n\
         PRINT (VIA 1)\n"
    )
}

#[test]
fn a_nested_recursions_lookups_probe_no_deeper_with_depth() {
    // Each level is a home seeded from its parent's memo, and lays a decision under the block's
    // key before its nested call, while the entry's key, laid at the first level, is found at
    // every level: a memo replaces a key's entry in place, so the entry's lookup reaches no
    // deeper as the levels above lay more.
    use crate::scope::PROBED;
    let source = nested_deepening();
    let [shallow, deep] = [8, 32].map(|depth| {
        PROBED.with(|probed| probed.set(0));
        let (ran, _) = ran(depth, &source);
        assert_eq!(ran.printed, format!("{}7", "a\n".repeat(depth as usize)));
        (
            KEPT.with(std::cell::Cell::get),
            PROBED.with(std::cell::Cell::get),
        )
    });
    assert!(deep.0 > shallow.0, "every level lays: {shallow:?} {deep:?}");
    assert_eq!(
        shallow.1, deep.1,
        "the deepest lookup: {shallow:?} {deep:?}"
    );
}

#[test]
fn a_memo_whose_keys_collide_finds_each_entry() {
    // With every key's hash masked to a few bits, or to none, keys share branches and then lie
    // side by side past the last bit: each run decides as often as with whole hashes.
    use crate::scope::HASH_MASK;
    let sources = [
        format!("{MK}{TELL}{}", under_enclosers("TELL", true)),
        ping_pong("(LET n = (OTHER x (ATTR link next))) (PRINT (PEEK x)) (n)"),
        nested_deepening(),
    ];
    for source in &sources {
        let whole = ran(8, source);
        for mask in [0b11, 0] {
            HASH_MASK.with(|hash| hash.set(mask));
            let masked = ran(8, source);
            HASH_MASK.with(|hash| hash.set(u64::MAX));
            assert_eq!(masked.0.printed, whole.0.printed);
            assert_eq!(masked.1, whole.1, "walks with the hash masked to {mask:#b}");
        }
    }
}

#[test]
fn a_block_hit_reads_its_enclosers_current_binding() {
    // `SWAP` binds `Elt` to `{a :Number}` and `{a :Str}` in turn, so `a`, the encloser's, takes a
    // number and a string in turn, and the block's `DUO` reads it. From the third hop the block's
    // seam hits by identity, its layers laid over the encloser's current binding layer: it walks
    // no more past the second hop, and prints as an unmemoized run does.
    let source = format!(
        "{SHAPE_DUO_SWAP}EXPR FOR ALL #[Elt] #(LOOP x :Elt link :Link) -> Number = #(\n  \
         LET a = (SHAPE x)\n  \
         PRINT (USING empty SCOPE (\n    LET r = (NEXT 1)\n    DUO r.zero a\n  ))\n  \
         LOOP (SWAP x) (ATTR link next)\n)\n\
         EXPR FOR ALL #[Elt] #(LOOP x :Elt end :Null) -> Number = #(7)\n\
         EXPR FOR ALL #[Item] #(PAIR y :Item) -> Number = #(LOOP {{a = y}} chain)\n\
         EXPR #(VIA y :Any) -> Any = #(PAIR y)\n\
         PRINT (VIA 1)\n"
    );
    let block = |depth| segment_of(depth, &source, crate::scope::ShapeKind::Block, "r");
    let [shallow, deep] = [4, 8].map(|depth| {
        let printed = format!("{}7", "nn\nns\n".repeat(depth as usize / 2));
        UNMEMOIZED.with(|off| off.set(true));
        assert_eq!(ran(depth, &source).0.printed, printed, "unmemoized");
        UNMEMOIZED.with(|off| off.set(false));
        walks_at(depth, &source, &printed, [block(depth)])
    });
    assert!(shallow[0] >= 1, "{shallow:?}");
    assert_eq!(shallow, deep, "walks at depth 4 and 8");
}

#[test]
fn a_seam_binding_a_module_under_a_repeating_binding_decides_once() {
    // Each hop's seam binds `h`, a module over `q`, alike, under the same entry binding: its
    // binding layer admits by identity of the module binder too, so it hits from the second hop.
    let source = format!(
        "{PEEK}EXPR FOR ALL #[Elt] #(LOOP x :Elt link :Link) -> Number = #(\n  \
         LET q = (NEXT 1)\n  \
         MODULE h OVER #[q] = ((LET zero = q.zero) \
         (LET succ = (FN :{{x :Number}} -> Number = #(x))))\n  \
         LET g = (FN :{{p :Counter}} -> Any = #(PEEK {{c = p.zero, e = x}}))\n  \
         LET v = (g {{p = h}})\n  PRINT v\n  LOOP x (ATTR link next)\n)\n\
         EXPR FOR ALL #[Elt] #(LOOP x :Elt end :Null) -> Number = #(7)\n\
         EXPR FOR ALL #[Item] #(PAIR y :Item) -> Number = #(LOOP {{a = y}} chain)\n\
         PRINT (PAIR 1)\n"
    );
    let body = |depth| segment_of(depth, &source, crate::scope::ShapeKind::Callable, "q");
    let [shallow, deep] = [4, 16].map(|depth| {
        let printed = format!("{}7", "a\n".repeat(depth as usize));
        walks_at(depth, &source, &printed, [body(depth)])
    });
    assert_eq!(shallow, deep, "walks at depth 4 and 16");
}
