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

/// The callable of `program` whose slots bind `slots`, by name, in slot order.
fn callable<'graph>(program: &Program<'graph>, names: &[&str]) -> &'graph BodyShape<'graph> {
    let named = |shape: &BodyShape<'_>| {
        (0..shape.slots())
            .map(|index| {
                let symbol = shape.slot_name(crate::scope::Slot(index as u32)).symbol();
                program.symbols().display(symbol).to_string()
            })
            .collect::<Vec<_>>()
    };
    let (_, found) = (program.shape().nested_shapes().iter())
        .find(|(_, nested)| nested.kind() == ShapeKind::Callable && named(nested) == names)
        .unwrap_or_else(|| panic!("a callable binds {names:?}"));
    found
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
