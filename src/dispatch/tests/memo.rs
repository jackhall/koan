//! The decision memo: a cell running a body over the bindings an earlier cell of its chain decided
//! reuses that decision, kept in the region the chain's results are built in, rather than walking.

use proptest::prelude::*;

use crate::program::activation_store::{DECIDED, KEPT, UNMEMOIZED};
use crate::values::BuildBudget;

use super::caching::{Ran, ran_under};

/// `MAKE`, a recursive functor the load never unfolds, whose result is content the run produced;
/// `PICK`, whose overloads tell a carrier of numbers from one of strings; and `NEXT`, which makes
/// `ints` or `words` over the chain by the type of what it is handed.
const MAKE: &str = "EXPR #(MAKE m :Counter link :Link) -> Counter = #(MAKE m (ATTR link next))\n\
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
fn a_statement_binding_that_changes_redecides_from_that_statement() {
    // Each hop reads its entry's decision, and decides `r`'s statement anew.
    for depth in [2, 8] {
        let (ran, decided) = ran(depth, &alternating());
        assert_eq!(ran.printed, alternated(depth));
        assert_eq!(
            decided,
            1 + depth as usize,
            "the entry once, `r` at each hop"
        );
    }
}

#[test]
fn a_fresh_loop_carrying_its_decision_memo_holds_its_heap_flat() {
    // The loop's home keeps a decision at each hop's `r` until its budget is spent, then only
    // reads: the hops past it still find what it kept, and lay nothing in the home.
    let runs = [40, 80].map(|depth| {
        let (ran, _) = ran(depth, &alternating());
        assert_eq!(ran.printed, alternated(depth));
        (ran.tiers.region, KEPT.with(std::cell::Cell::get))
    });
    assert_eq!(runs[0], runs[1]);
    assert_eq!(runs[0].1, 32, "the budget's worth of decisions");
}

/// `PING` and `PONG`, which call each other down the chain over a record whose type the run
/// composed, through `call`: a tail call where it is the body's last statement, a nested one where
/// its value is bound first.
fn ping_pong(call: &str) -> String {
    let body = |other: &str| call.replace("OTHER", other);
    let (ping, pong) = (body("PONG"), body("PING"));
    format!(
        "{PEEK}EXPR FOR ALL #[Elt] #(PING x :Elt link :Link) -> Number = #({ping})\n\
         EXPR FOR ALL #[Elt] #(PING x :Elt end :Null) -> Number = #(7)\n\
         EXPR FOR ALL #[Elt] #(PONG x :Elt link :Link) -> Number = #({pong})\n\
         EXPR FOR ALL #[Elt] #(PONG x :Elt end :Null) -> Number = #(7)\n\
         EXPR FOR ALL #[Item] #(PAIR y :Item) -> Number = #(PING {{a = y}} chain)\n\
         PRINT (PAIR 1)\n"
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
    // `RUNS` keys a settlement on `k`, which it passes on unchanged: the chain walks under `ints`,
    // then its end hops to a walk under `words`. `r`'s statement binds the same content at every
    // hop, and decides once under each settlement; the end that hops between them decides at its
    // entry, binding the names it calls `RUNS` over.
    let source = format!(
        "{BOTH}EXPR #(RUNS k :Counter j :Counter link :Link stop :Any) -> Number = \
         #((PRINT (PICK k.zero)) (LET r = (MAKE ints chain)) (PRINT (BOTH r.zero AND k.zero)) \
         (RUNS k j (ATTR link next) stop))\n\
         EXPR #(RUNS k :Counter j :Counter end :Null stop :Link) -> Number = \
         #(RUNS j k stop null)\n\
         EXPR #(RUNS k :Counter j :Counter end :Null stop :Null) -> Number = #(7)\n\
         PRINT (RUNS ints words chain chain)\n"
    );
    let (ran, decided) = ran(4, &source);
    assert_eq!(
        ran.printed,
        format!("{}{}7", "n\nnn\n".repeat(4), "1\nnn\n".repeat(4))
    );
    assert_eq!(ran.tiers.settled, 4, "`MAKE`'s two and `RUNS`'s two");
    assert_eq!(
        decided, 3,
        "`r` once under each settlement, and the hop between the walks once"
    );
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
    assert_eq!(decided, 8 + 2, "`r` at each hop, and `g` once per `r`");
}

/// A chain of nodes, the `at`-th holding `hops[at]`: whether its statement binds `r` to `ints`, and
/// whether its block surfaces `ints`, else `words`; then `WALK`, which binds both at each hop and
/// decides over them, its entry binding the same type throughout.
fn walked(hops: &[(bool, bool)]) -> String {
    let tag = |ints: bool| if ints { "1" } else { "\"s\"" };
    let mut source = format!(
        "{PEEK}NEWTYPE Node = :{{next :Any, s :Any, b :Any}}\n\
         EXPR FOR ALL #[Elt] #(WALK e :Elt node :Node) -> Number = #(\n  \
         LET r = (NEXT (ATTR node s))\n  PRINT (PICK r.zero)\n  \
         LET q = (NEXT (ATTR node b))\n  PRINT (USING (q :! Counter) SCOPE (PEEK zero))\n  \
         PRINT (PEEK e)\n  WALK e (ATTR node next)\n)\n\
         EXPR FOR ALL #[Elt] #(WALK e :Elt end :Null) -> Number = #(7)\n\
         EXPR FOR ALL #[Item] #(PAIR y :Item) -> Number = #(WALK {{a = y}} node0)\n\
         LET node{} = null\n",
        hops.len()
    );
    for (at, (statement, block)) in hops.iter().enumerate().rev() {
        source.push_str(&format!(
            "LET node{at} = (Node {{next = node{}, s = {}, b = {}}})\n",
            at + 1,
            tag(*statement),
            tag(*block)
        ));
    }
    source.push_str("PRINT (PAIR 1)\n");
    source
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
        hops in prop::collection::vec((any::<bool>(), any::<bool>()), 1..8),
    ) {
        let source = walked(&hops);
        let (memoized, decided) = ran(0, &source);
        UNMEMOIZED.with(|off| off.set(true));
        let (scratch, walks) = ran(0, &source);
        UNMEMOIZED.with(|off| off.set(false));
        prop_assert_eq!(&memoized.printed, &scratch.printed, "in\n{}", source);
        prop_assert!(!memoized.printed.contains("error"), "{}", memoized.printed);
        prop_assert!(decided <= walks, "{} walks memoized, {} from scratch", decided, walks);
    }
}
