//! Activations: what a frame bound to content the run produced decides in its own activation, at
//! its entry and at each statement seam a fact waits on.

use crate::parse::ExpressionPart;
use crate::program::Program;
use crate::program::activation_store::DECIDED;
use crate::scope::{BodyShape, Narrowing, Segment, ShapeKind, Site};

use super::caching::{Ran, ran_under};
use super::statics::{loaded, slot};
use crate::values::BuildBudget;

/// Two overloads of `PICK`, one at `Any` and one at `Str`: the load names no record type.
const PICK: &str = "EXPR #(PICK x :Any) -> Str = #(\"n\")\n\
                    EXPR #(PICK x :Str) -> Number = #(1)\n";

/// `WRAP`, whose `PICK x` waits on `Elt`, and `PAIR`, which hands it a record it builds from its
/// own argument: a type the run composes, which keys no settlement.
const WRAP: &str = "EXPR FOR ALL #[Elt] #(WRAP x :Elt) -> Any = #((LET p = (PICK x)) (p))\n\
                    EXPR FOR ALL #[Item] #(PAIR y :Item) -> Any = #(WRAP {a = y, b = y})\n";

/// Run `source`, counting the binding events that walked.
fn ran(source: &str) -> (Ran, usize) {
    DECIDED.with(|decided| decided.set(0));
    let ran = ran_under(0, source, BuildBudget::default());
    (ran, DECIDED.with(std::cell::Cell::get))
}

/// The callable of `program` whose slots bind `names`, in slot order.
fn callable<'graph>(program: &Program<'graph>, names: &[&str]) -> &'graph BodyShape<'graph> {
    nested(program, ShapeKind::Callable, names)
}

/// The body of `kind` in `program` whose slots bind `names`, in slot order.
fn nested<'graph>(
    program: &Program<'graph>,
    kind: ShapeKind,
    names: &[&str],
) -> &'graph BodyShape<'graph> {
    let named = |shape: &BodyShape<'_>| {
        (0..shape.slots())
            .map(|index| {
                let symbol = shape.slot_name(crate::scope::Slot(index as u32)).symbol();
                program.symbols().display(symbol).to_string()
            })
            .collect::<Vec<_>>()
    };
    (program.shapes().bodies().iter())
        .find(|nested| nested.kind() == kind && named(nested) == names)
        .unwrap_or_else(|| {
            let all: Vec<_> = (program.shapes().bodies().iter())
                .map(|nested| named(nested))
                .collect();
            panic!("a {kind:?} body binds {names:?}: {all:?}")
        })
}

/// The site of `WRAP`'s `PICK x`, and whether the load selected it.
fn wraps_pick(source: &str) -> (Segment, Site, bool) {
    loaded(&super::caching::program(0, source), |program| {
        let wrap = callable(program, &["p", "x", "Elt"]);
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

thread_local! {
    /// Each use a frame found selected in its facts, by its body's segment and its site.
    static CHOSEN: std::cell::RefCell<Vec<(Segment, Site)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Note a use a frame found selected.
fn chose(segment: Segment, site: Site) {
    CHOSEN.with(|chosen| chosen.borrow_mut().push((segment, site)));
}

#[test]
fn a_frame_bound_to_run_content_decides_its_dependent_units_in_its_activation() {
    // `PAIR`'s record is a type the run composed, so `WRAP` seeks no settlement and runs under the
    // root; its entry binds `Elt` in its activation, where `PICK x` is selected.
    let source = format!("{PICK}{WRAP}PRINT (PAIR 1)\n");
    let (segment, site, selected) = wraps_pick(&source);
    assert!(!selected, "the load selects nothing at `PICK x`");
    CHOSEN.with(|chosen| chosen.borrow_mut().clear());
    crate::dispatch::evaluate::CHOSE.with(|hook| hook.set(Some(chose)));
    let (ran, decided) = ran(&source);
    crate::dispatch::evaluate::CHOSE.with(|hook| hook.set(None));
    assert_eq!(ran.printed, "n");
    assert_eq!(
        (ran.tiers.sought, ran.tiers.settled),
        (1, 1),
        "`PAIR` alone seeks"
    );
    assert_eq!(decided, 1, "`WRAP`'s entry decides once");
    let chosen = CHOSEN.with(|chosen| chosen.take());
    assert!(chosen.contains(&(segment, site)), "{chosen:?}");
}

#[test]
fn run_content_bindings_grow_the_agent_tier_by_nothing() {
    // Each `WRAP` frame binds a record type the run composed and decides in its activation, whose
    // types rest in its frame's tiers: the agent tier holds what `PAIR`'s one settlement rests.
    let (once, decided_once) = ran(&format!("{PICK}{WRAP}PRINT (PAIR 1)\n"));
    let (twice, decided_twice) = ran(&format!(
        "{PICK}{WRAP}LET first = (PAIR 1)\nPRINT (PAIR 2)\n"
    ));
    assert_eq!((decided_once, decided_twice), (1, 2));
    assert_eq!((once.tiers.settled, twice.tiers.settled), (1, 1));
    assert_eq!(once.tiers.agent, twice.tiers.agent);
}

/// `WALK`, a `Fresh` loop over the chain whose every hop calls `WRAP` on load content: a number.
const WALK: &str = "EXPR #(WALK link :Link) -> Number = \
                    #((LET seen = (WRAP 1)) (WALK (ATTR link next)))\n\
                    EXPR #(WALK end :Null) -> Number = #(7)\n";

#[test]
fn a_loop_calling_a_keyed_callee_on_load_content_holds_o1_settlements() {
    // `WRAP 1` keys `Elt := Number`, load content: every hop finds the one settlement, and no
    // frame decides anything in its activation.
    for depth in [1, 4, 16] {
        DECIDED.with(|decided| decided.set(0));
        let ran = ran_under(
            depth,
            &format!("{PICK}{WRAP}{WALK}PRINT (WALK chain)\n"),
            BuildBudget::default(),
        );
        assert_eq!(ran.printed, "7");
        assert_eq!((ran.tiers.sought, ran.tiers.settled), (depth as usize, 1));
        assert_eq!(DECIDED.with(std::cell::Cell::get), 0);
    }
}

/// `MAKE`, a recursive functor the load never unfolds, and `USE`, which binds what it returns at
/// a statement: a module the facts hold only as a leaf, whose `zero` `PICK` reads.
const MAKE: &str = "EXPR #(MAKE m :Counter link :Link) -> Counter = #(MAKE m (ATTR link next))\n\
                    EXPR #(MAKE m :Counter end :Null) -> Counter = #(m)\n\
                    EXPR #(PICK x :Number) -> Str = #(\"n\")\n\
                    EXPR #(PICK x :Str) -> Number = #(1)\n\
                    EXPR #(USE n :Number) -> Any = \
                    #((LET r = (MAKE ints chain)) (LET p = (PICK r.zero)) (p))\n";

#[test]
fn a_statement_seam_decides_the_units_it_wakes_in_the_activation() {
    // The load leaves `PICK r.zero` open; once `r` is bound, `USE`'s activation decides it.
    let source = format!("{MAKE}PRINT (USE 1)\n");
    let (segment, site, selected) = loaded(&super::caching::program(2, &source), |program| {
        let use_ = callable(program, &["r", "p", "n"]);
        let Some(ExpressionPart::Expression(node)) = use_.rhs(slot(program, use_, "p")) else {
            panic!("`p` is bound to a call");
        };
        let site = use_.site_of_node(node.reference());
        let narrowing = program.facts().use_statics(use_, site).narrowing;
        (
            use_.segment(),
            site,
            matches!(narrowing, Narrowing::Selected(_)),
        )
    });
    assert!(!selected, "the load selects nothing at `PICK r.zero`");
    CHOSEN.with(|chosen| chosen.borrow_mut().clear());
    crate::dispatch::evaluate::CHOSE.with(|hook| hook.set(Some(chose)));
    DECIDED.with(|decided| decided.set(0));
    let ran = ran_under(2, &source, BuildBudget::default());
    crate::dispatch::evaluate::CHOSE.with(|hook| hook.set(None));
    assert_eq!(ran.printed, "n");
    assert_eq!(
        DECIDED.with(std::cell::Cell::get),
        1,
        "`r`'s statement decides once"
    );
    let chosen = CHOSEN.with(|chosen| chosen.take());
    assert!(chosen.contains(&(segment, site)), "{chosen:?}");
}

/// A message, its location dropped.
fn unlocated(message: &str) -> &str {
    message.split_once(": ").map_or(message, |(_, rest)| rest)
}

#[test]
fn a_statement_binding_that_cannot_type_faults_where_the_load_would() {
    // `r` is `words` at run, so `PICK r.zero` reads a string, which no overload admits: the
    // activation refuses at `r`'s statement, worded as the load words the use with `r` declared.
    let picks = "EXPR #(PICK x :Number) -> Str = #(\"n\")\n\
                 EXPR #(PICK x :Bool) -> Number = #(1)\n";
    let make = MAKE.lines().take(2).collect::<Vec<_>>().join("\n");
    let run = format!(
        "{make}\n{picks}EXPR #(USE n :Number) -> Any = \
         #((LET r = (MAKE words chain)) (LET p = (PICK r.zero)) (p))\nPRINT (USE 1)\n"
    );
    let declared = format!(
        "{make}\n{picks}EXPR #(USE n :Number) -> Any = #((LET p = (PICK words.zero)) (p))\n\
         PRINT (USE 1)\n"
    );
    let ran = ran_under(2, &run, BuildBudget::default());
    let fault = ran.printed.strip_prefix("error: ").expect("the run faults");
    let refused = super::run(&super::caching::program(2, &declared));
    let refused = refused
        .strip_prefix("load: ")
        .expect("the declared program refuses");
    assert_eq!(unlocated(fault), unlocated(refused));
    assert!(
        unlocated(fault).starts_with("no overload of `PICK _`"),
        "{fault}"
    );
}

#[test]
fn a_module_body_reads_its_frames_entry_decisions() {
    // `INNER`'s `m` is bound to what `MAKE` returned, so its entry decides `h`'s `PICK m.zero`,
    // which the module body, running inline in `INNER`'s frame, reads.
    let inner = "EXPR #(INNER m :Counter) -> Any = \
                 #((MODULE h OVER #[m] = (LET p = (PICK m.zero))) (h.p))\n\
                 EXPR #(OUTER n :Number) -> Any = #((LET r = (MAKE ints chain)) (INNER r))\n";
    let make = MAKE.lines().take(4).collect::<Vec<_>>().join("\n");
    let source = format!("{make}\n{inner}PRINT (OUTER 1)\n");
    let (segment, site, selected) = loaded(&super::caching::program(2, &source), |program| {
        let body = nested(program, ShapeKind::Module, &["p"]);
        let Some(ExpressionPart::Expression(node)) = body.rhs(slot(program, body, "p")) else {
            panic!("`p` is bound to a call");
        };
        let site = body.site_of_node(node.reference());
        let narrowing = program.facts().use_statics(body, site).narrowing;
        (
            body.segment(),
            site,
            matches!(narrowing, Narrowing::Selected(_)),
        )
    });
    assert!(!selected, "the load selects nothing at `PICK m.zero`");
    CHOSEN.with(|chosen| chosen.borrow_mut().clear());
    crate::dispatch::evaluate::CHOSE.with(|hook| hook.set(Some(chose)));
    let ran = ran_under(2, &source, BuildBudget::default());
    crate::dispatch::evaluate::CHOSE.with(|hook| hook.set(None));
    assert_eq!(ran.printed, "n");
    let chosen = CHOSEN.with(|chosen| chosen.take());
    assert!(chosen.contains(&(segment, site)), "{chosen:?}");
}

/// The body of `kind` in `program` binding a slot named `name`.
fn holding<'graph>(
    program: &Program<'graph>,
    kind: ShapeKind,
    name: &str,
) -> &'graph BodyShape<'graph> {
    let binds = |shape: &BodyShape<'_>| {
        (0..shape.slots()).any(|index| {
            let symbol = shape.slot_name(crate::scope::Slot(index as u32)).symbol();
            program.symbols().display(symbol).to_string() == name
        })
    };
    (program.shapes().bodies().iter())
        .find(|shape| shape.kind() == kind && binds(shape))
        .unwrap_or_else(|| panic!("a {kind:?} body binds `{name}`"))
}

/// The segment and site of the call `p` is bound to in the block binding it, and whether the load
/// selected it.
fn blocks_pick(source: &str) -> (Segment, Site, bool) {
    loaded(&super::caching::program(2, source), |program| {
        let block = holding(program, ShapeKind::Block, "p");
        let Some(ExpressionPart::Expression(node)) = block.rhs(slot(program, block, "p")) else {
            panic!("`p` is bound to a call");
        };
        let site = block.site_of_node(node.reference());
        let narrowing = program.facts().use_statics(block, site).narrowing;
        (
            block.segment(),
            site,
            matches!(narrowing, Narrowing::Selected(_)),
        )
    })
}

/// Run `source` over the caching prelude and a chain of two links, counting the binding events
/// that walked and noting each use a frame found selected.
fn ran_choosing(source: &str) -> (Ran, usize, Vec<(Segment, Site)>) {
    CHOSEN.with(|chosen| chosen.borrow_mut().clear());
    crate::dispatch::evaluate::CHOSE.with(|hook| hook.set(Some(chose)));
    DECIDED.with(|decided| decided.set(0));
    let ran = ran_under(2, source, BuildBudget::default());
    crate::dispatch::evaluate::CHOSE.with(|hook| hook.set(None));
    let decided = DECIDED.with(std::cell::Cell::get);
    (ran, decided, CHOSEN.with(|chosen| chosen.take()))
}

#[test]
fn a_blocks_statement_seam_decides_in_the_block() {
    // `r` is bound at a statement of the block `USING empty` runs: the block's own cell decides
    // `PICK r.zero` there.
    let make = MAKE.lines().take(4).collect::<Vec<_>>().join("\n");
    let source = format!(
        "{make}\nEXPR #(USE n :Number) -> Any = #(USING empty SCOPE (\n  \
         LET r = (MAKE ints chain)\n  LET p = (PICK r.zero)\n  p\n))\nPRINT (USE 1)\n"
    );
    let (segment, site, selected) = blocks_pick(&source);
    assert!(!selected, "the load selects nothing at `PICK r.zero`");
    let (ran, decided, chosen) = ran_choosing(&source);
    assert_eq!(ran.printed, "n");
    assert_eq!(decided, 1, "`r`'s statement decides once");
    assert!(chosen.contains(&(segment, site)), "{chosen:?}");
}

#[test]
fn a_using_body_over_run_content_decides_at_its_entry() {
    // The block surfaces `Carrier` from what `MAKE` returned, opaque: `USE`'s statement binding
    // `r` decides first, leaving `PICK zero` open, and the block's entry decides it at `Any`.
    let make = MAKE.lines().take(2).collect::<Vec<_>>().join("\n");
    let source = format!(
        "{make}\n{PICK}EXPR #(USE n :Number) -> Any = #(\n  LET r = (MAKE ints chain)\n  \
         USING (r :! Counter) SCOPE (\n    LET p = (PICK zero)\n    p\n  )\n)\n\
         PRINT (USE 1)\n"
    );
    let (segment, site, selected) = blocks_pick(&source);
    assert!(!selected, "the load selects nothing at `PICK zero`");
    let (ran, decided, chosen) = ran_choosing(&source);
    assert_eq!(ran.printed, "n");
    assert_eq!(decided, 2, "`r`'s statement, then the block's entry");
    assert!(chosen.contains(&(segment, site)), "{chosen:?}");
}
