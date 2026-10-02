//! Priority classes: the worked examples the dispatch design states, pinned as the edges of the
//! ranking laws in [`properties`](super::properties).
//!
//! No law says which candidate a particular pair of heads selects, so each selection example here is
//! the design's own, verbatim: `SHOW` decided by its first class, `MOVE` by its second, `PAIR`
//! telling one variable from two, `TAKE` reading an unadmitted class's variable as its bound. The
//! rest pin the normalizer, the ranking's place in a shape's identity and rendering, the verdict
//! table's reuse, and the two signature relations a ranking refuses in.

use crate::memory::{Bump, BumpAllocator};
use crate::symbols::{BinderSymbol, KeywordSymbol, SymbolInterner, TypeSymbol};

use crate::type_lattice::handle::{Handle, KType, TypeHandle};
use crate::type_lattice::node::TypeNode;
use crate::type_lattice::order::fits;
use crate::type_lattice::ranking::{Verdict, admit_by_class, judge_by_class, select_by_class};
use crate::type_lattice::registry::TypeRegistry;
use crate::type_lattice::render::display_handle;
use crate::type_lattice::schema::{SchemaDraft, SigOrigin};
use crate::type_lattice::shape::{DispatchTokenElement, RawRank, Specificity, dense_classes};
use crate::type_lattice::sig_relations::{FitsFailure, shape_specificity, sig_fits};
use crate::type_lattice::unify::Interval;

/// Anything under `kt`, read raw.
fn within(kt: Handle) -> Interval<Handle> {
    Interval {
        lower: Handle::NEVER,
        upper: kt,
    }
}

/// One position of a test head: a keyword by its text, or a slot's type.
enum Part<'a> {
    Kw(&'a str),
    Slot(Handle),
}
use Part::{Kw, Slot};

/// A registry, an interner and a region, and the door that interns a head over them.
struct World<'r> {
    region: BumpAllocator<'r>,
    types: TypeRegistry<'r>,
    symbols: SymbolInterner,
}

impl<'r> World<'r> {
    fn new(region: BumpAllocator<'r>) -> Self {
        World {
            region,
            types: TypeRegistry::in_region(region),
            symbols: SymbolInterner::new(),
        }
    }

    /// A type name for a quantifier group.
    fn name(&self, text: &str) -> TypeSymbol {
        TypeSymbol::declared(text, &self.symbols).expect("a Type token")
    }

    /// The `index`-th variable of a group, bounded by `Any`.
    fn var(&self, index: usize) -> Handle {
        self.types.quantified(index, KType::ANY).raw()
    }

    fn union(&self, members: &[Handle]) -> Handle {
        self.types.union_of(self.region, members)
    }

    /// The shape `FOR ALL #[<group>] #(<parts>) -> Any` under `classes`.
    fn head(&self, group: &[&str], parts: &[Part<'_>], classes: &[u8]) -> Handle {
        self.head_to(group, parts, classes, Handle::ANY)
    }

    /// The shape `FOR ALL #[<group>] #(<parts>) -> <ret>` under `classes`, each variable bounded by
    /// `Any`.
    fn head_to(&self, group: &[&str], parts: &[Part<'_>], classes: &[u8], ret: Handle) -> Handle {
        let names: Vec<TypeSymbol> = group.iter().map(|text| self.name(text)).collect();
        let bounds = vec![KType::ANY; names.len()];
        let elements: Vec<DispatchTokenElement> = parts
            .iter()
            .map(|part| match part {
                Kw(text) => DispatchTokenElement::Keyword(
                    KeywordSymbol::declared(text, &self.symbols).expect("a keyword token"),
                ),
                Slot(kt) => DispatchTokenElement::Slot(*kt),
            })
            .collect();
        self.types
            .shape_group(self.region, &names, &bounds, &elements, classes, ret)
            .0
    }

    /// `FN :{v :<param>} -> Null`.
    fn handler(&self, param: Handle) -> Handle {
        let v = BinderSymbol::declared("v", &self.symbols).expect("a bindable token");
        self.types
            .function_type(self.region, &[(v, param)], KType::NULL.raw())
    }

    /// `shape`'s verdict over `arguments`, and its intervals.
    fn judge(
        &self,
        shape: Handle,
        arguments: &[Interval<Handle>],
    ) -> (Verdict, Option<Vec<Interval<Handle>>>) {
        let judged = judge_by_class(&self.types, self.region, shape, arguments);
        let intervals = judged
            .intervals
            .map(|intervals| intervals.iter().map(|interval| interval.raw()).collect());
        (judged.verdict, intervals)
    }

    /// The survivors of selection over `shapes`.
    fn select(&self, shapes: &[Handle]) -> Vec<usize> {
        select_by_class(&self.types, self.region, shapes).to_vec()
    }
}

#[test]
fn a_written_ranking_normalizes_to_dense_classes() {
    let bump = Bump::new();
    let n = RawRank::Numbered;
    let u = RawRank::Unnumbered;
    let cases: [(&[RawRank], &[u8]); 6] = [
        (&[n(2), n(1)], &[1, 0]),
        (&[n(20), n(10)], &[1, 0]),
        (&[u, u], &[0, 1]),
        (&[n(1), u], &[0, 1]),
        (&[u, n(1)], &[1, 0]),
        (&[n(1), n(1)], &[0, 0]),
    ];
    for (raw, dense) in cases {
        assert_eq!(dense_classes(&bump, raw), dense, "{raw:?}");
    }
}

/// `:(EXPR #(MOVE 2 :Any TO 1 :Any) -> Any)` and `:(EXPR #(MOVE 20 :Any TO 10 :Any) -> Any)` are
/// one type, and `:(EXPR #(MOVE _ :Any TO _ :Any) -> Any)` is another; written order spelled out
/// is written order.
#[test]
fn a_ranking_is_part_of_a_shapes_identity() {
    let bump = Bump::new();
    let world = World::new(&bump);
    let parts = [Kw("MOVE"), Slot(Handle::ANY), Kw("TO"), Slot(Handle::ANY)];
    let raw = |ranks: &[RawRank]| dense_classes(&bump, ranks);
    let two_one = world.head(
        &[],
        &parts,
        raw(&[RawRank::Numbered(2), RawRank::Numbered(1)]),
    );
    let twenty_ten = world.head(
        &[],
        &parts,
        raw(&[RawRank::Numbered(20), RawRank::Numbered(10)]),
    );
    let written = world.head(&[], &parts, &[]);
    assert_eq!(two_one, twenty_ten);
    assert_ne!(two_one, written);
    assert_eq!(world.head(&[], &parts, &[0, 1]), written);
    let TypeNode::ExpressionShape { classes, .. } = world.types.node(written) else {
        unreachable!("the shape door interns a shape")
    };
    assert!(classes.is_empty());

    let render = |kt| display_handle(kt, &world.types, &world.symbols).to_string();
    assert_eq!(render(two_one), ":(EXPR #(MOVE 2 :Any TO 1 :Any) -> Any)");
    assert_eq!(render(written), ":(EXPR #(MOVE _ :Any TO _ :Any) -> Any)");
}

/// `FOR ALL #[Elt] #(PAIR x :(LIST OF Elt) WITH y :(LIST OF Elt))` in written order solves `Elt`
/// from `x` and admits `y` against it; ranked `1 … 1`, the two slots solve jointly.
#[test]
fn a_keyworded_call_solves_class_by_class() {
    let bump = Bump::new();
    let world = World::new(&bump);
    let list_of_elt = world.types.list(world.var(0));
    let parts = [Kw("PAIR"), Slot(list_of_elt), Kw("WITH"), Slot(list_of_elt)];
    let written = world.head(&["Elt"], &parts, &[]);
    let joint = world.head(&["Elt"], &parts, &[0, 0]);
    let numbers = world.types.list(KType::NUMBER.raw());
    let mixed = world
        .types
        .list(world.union(&[KType::NUMBER.raw(), KType::STR.raw()]));
    let admits = |shape, arguments: &[Handle]| {
        admit_by_class(&world.types, world.region, shape, arguments).is_some()
    };
    assert!(!admits(written, &[numbers, mixed]));
    assert!(admits(written, &[mixed, numbers]));
    assert!(admits(joint, &[numbers, mixed]));
    assert!(admits(joint, &[mixed, numbers]));
    assert_eq!(
        admit_by_class(&world.types, world.region, written, &[mixed, numbers]),
        Some(&[world.union(&[KType::NUMBER.raw(), KType::STR.raw()])][..])
    );
}

/// `SHOW 1 WITH "s"`: the first class decides.
#[test]
fn show_is_decided_by_its_first_class() {
    let bump = Bump::new();
    let world = World::new(&bump);
    let concrete = world.head(
        &[],
        &[
            Kw("SHOW"),
            Slot(KType::NUMBER.raw()),
            Kw("WITH"),
            Slot(Handle::ANY),
        ],
        &[],
    );
    let generic = world.head(
        &["Elt"],
        &[
            Kw("SHOW"),
            Slot(world.var(0)),
            Kw("WITH"),
            Slot(KType::STR.raw()),
        ],
        &[],
    );
    assert_eq!(world.select(&[concrete, generic]), [0]);
    assert_eq!(world.select(&[generic, concrete]), [1]);
}

/// `MOVE "s" TO 1`: the first class orders neither, so both pass to the second, which decides.
#[test]
fn move_is_decided_by_its_second_class() {
    let bump = Bump::new();
    let world = World::new(&bump);
    let narrow = world.head(
        &[],
        &[
            Kw("MOVE"),
            Slot(world.union(&[KType::STR.raw(), KType::BOOL.raw()])),
            Kw("TO"),
            Slot(KType::NUMBER.raw()),
        ],
        &[],
    );
    let wide = world.head(
        &[],
        &[
            Kw("MOVE"),
            Slot(world.union(&[KType::NUMBER.raw(), KType::STR.raw()])),
            Kw("TO"),
            Slot(Handle::ANY),
        ],
        &[],
    );
    assert_eq!(world.select(&[narrow, wide]), [0]);
    assert_eq!(
        shape_specificity(&world.types, world.region, narrow, wide),
        Specificity::StrictlyMore
    );
}

/// `PAIR 1 WITH 2`: one variable beats two. The one-variable head's `Elt`, solved from `x`, reads at
/// `y` as a stand-in over whatever a call binds it to, which the two-variable head's independent
/// `Second` does not lie under.
#[test]
fn pair_prefers_one_variable_to_two() {
    let bump = Bump::new();
    let world = World::new(&bump);
    let one = world.head(
        &["Elt"],
        &[
            Kw("PAIR"),
            Slot(world.var(0)),
            Kw("WITH"),
            Slot(world.var(0)),
        ],
        &[],
    );
    let two = world.head(
        &["First", "Second"],
        &[
            Kw("PAIR"),
            Slot(world.var(0)),
            Kw("WITH"),
            Slot(world.var(1)),
        ],
        &[],
    );
    assert_eq!(world.select(&[one, two]), [0]);
    assert_eq!(world.select(&[two, one]), [1]);
    assert_eq!(
        shape_specificity(&world.types, world.region, one, one),
        Specificity::Equal
    );
}

/// *Fits* over static types: a later class reads an earlier variable at its reach
/// interval, so `PAIR` in written order refuses a candidate whose second slot a call may fill with
/// a type the first did not carry, and `APPLY` admits one whose second slot lies above the first's
/// contravariant contribution.
#[test]
fn a_later_class_reads_an_earlier_variable_at_its_interval() {
    let bump = Bump::new();
    let world = World::new(&bump);
    let below = |a, b| fits(&world.types, world.region, a, b);
    let number_or_str = world.union(&[KType::NUMBER.raw(), KType::STR.raw()]);
    let pair = |group: &[&str], x, y, classes: &[u8]| {
        world.head_to(
            group,
            &[Kw("PAIR"), Slot(x), Kw("WITH"), Slot(y)],
            classes,
            KType::STR.raw(),
        )
    };
    let wide = pair(&[], number_or_str, number_or_str, &[]);
    assert!(!below(
        pair(&["Elt"], world.var(0), world.var(0), &[]),
        wide
    ));
    assert!(below(
        pair(&["Elt"], world.var(0), world.var(0), &[0, 0]),
        pair(&[], number_or_str, number_or_str, &[0, 0]),
    ));
    let apply = |group: &[&str], f, y| {
        world.head_to(
            group,
            &[Kw("APPLY"), Slot(f), Kw("TO"), Slot(y)],
            &[],
            KType::NULL.raw(),
        )
    };
    assert!(below(
        apply(&["Elt"], world.handler(world.var(0)), world.var(0)),
        apply(&[], world.handler(KType::NUMBER.raw()), KType::NUMBER.raw()),
    ));
}

/// `TAKE [1] WITH 1`: the first class admits neither way, so the generic head's `Elt` reads as its
/// bound at the second, where `Number` beats it.
#[test]
fn take_reads_an_unadmitted_variable_as_its_bound() {
    let bump = Bump::new();
    let world = World::new(&bump);
    let concrete = world.head(
        &[],
        &[
            Kw("TAKE"),
            Slot(world.union(&[KType::STR.raw(), world.types.list(KType::NUMBER.raw())])),
            Kw("WITH"),
            Slot(KType::NUMBER.raw()),
        ],
        &[],
    );
    let generic = world.head(
        &["Elt"],
        &[
            Kw("TAKE"),
            Slot(world.union(&[KType::NUMBER.raw(), world.types.list(world.var(0))])),
            Kw("WITH"),
            Slot(world.var(0)),
        ],
        &[],
    );
    assert_eq!(world.select(&[concrete, generic]), [0]);
}

/// Two heads under one ranking that no class orders are both survivors — an ambiguity for the
/// caller to report.
#[test]
fn heads_no_class_orders_both_survive() {
    let bump = Bump::new();
    let world = World::new(&bump);
    let left = world.head(
        &[],
        &[
            Kw("MIX"),
            Slot(KType::NUMBER.raw()),
            Kw("AND"),
            Slot(Handle::ANY),
        ],
        &[0, 0],
    );
    let right = world.head(
        &[],
        &[
            Kw("MIX"),
            Slot(Handle::ANY),
            Kw("AND"),
            Slot(KType::NUMBER.raw()),
        ],
        &[0, 0],
    );
    assert_eq!(world.select(&[left, right]), [0, 1]);
}

/// A pair's class verdicts are computed once, every class in one pass, and read back after.
#[test]
fn class_verdicts_are_recorded_and_reused() {
    let bump = Bump::new();
    let world = World::new(&bump);
    let a = world.head(
        &[],
        &[
            Kw("MOVE"),
            Slot(KType::STR.raw()),
            Kw("TO"),
            Slot(KType::NUMBER.raw()),
        ],
        &[],
    );
    let b = world.head(
        &[],
        &[Kw("MOVE"), Slot(Handle::ANY), Kw("TO"), Slot(Handle::ANY)],
        &[],
    );
    let _ = shape_specificity(&world.types, world.region, a, b);
    let (recorded, _) = world.types.verdict_tally();
    let _ = shape_specificity(&world.types, world.region, a, b);
    assert_eq!(world.types.verdict_tally().0, recorded);
}

/// A signature member ranked `2 … 1` is not satisfied by a module whose bucket is written-order.
#[test]
fn a_ranking_disagreement_refuses_in_fits() {
    let bump = Bump::new();
    let world = World::new(&bump);
    let parts = [Kw("MOVE"), Slot(Handle::ANY), Kw("TO"), Slot(Handle::ANY)];
    let ranked = world.head(&[], &parts, &[1, 0]);
    let written = world.head(&[], &parts, &[]);
    let signature = |member| {
        let mut draft = SchemaDraft::new(world.region);
        draft.origin = SigOrigin::Declared;
        draft.keyworded.push(world.types.declared(member));
        world.types.signature(world.region, draft).raw()
    };
    assert!(matches!(
        sig_fits(
            &world.types,
            world.region,
            signature(written),
            signature(ranked)
        ),
        Err(FitsFailure::RankingMismatch { .. })
    ));
    assert!(
        sig_fits(
            &world.types,
            world.region,
            signature(ranked),
            signature(ranked)
        )
        .is_ok()
    );
}

/// `FOR ALL #[Elt] #(ONLY x :Elt) -> Elt`: a bare slot admits whatever its argument carries, and
/// its interval is exact where the argument is.
#[test]
fn a_bare_variable_always_admits() {
    let bump = Bump::new();
    let world = World::new(&bump);
    let only = world.head_to(
        &["Elt"],
        &[Kw("ONLY"), Slot(world.var(0))],
        &[],
        world.var(0),
    );
    assert_eq!(
        world.judge(only, &[within(KType::NUMBER.raw())]),
        (Verdict::Always, Some(vec![within(KType::NUMBER.raw())]))
    );
    assert_eq!(
        world.judge(only, &[Interval::point(KType::NUMBER.raw())]),
        (
            Verdict::Always,
            Some(vec![Interval::point(KType::NUMBER.raw())])
        )
    );
}

/// `FOR ALL #[Elt] #(PAIR x :Elt WITH y :Elt)`: `y` is read at `Elt`'s least instance for *always*
/// and its greatest for *never*.
#[test]
fn pair_is_judged_through_its_first_class() {
    let bump = Bump::new();
    let world = World::new(&bump);
    let number_or_str = world.union(&[KType::NUMBER.raw(), KType::STR.raw()]);
    let parts = [
        Kw("PAIR"),
        Slot(world.var(0)),
        Kw("WITH"),
        Slot(world.var(0)),
    ];
    let pair = world.head(&["Elt"], &parts, &[]);
    let number = Interval::point(KType::NUMBER.raw());
    assert_eq!(world.judge(pair, &[number, number]).0, Verdict::Always);
    let either = within(number_or_str);
    assert_eq!(
        world.judge(pair, &[either, either]),
        (Verdict::Maybe, Some(vec![either]))
    );
    assert_eq!(
        world
            .judge(pair, &[number, Interval::point(KType::STR.raw())])
            .0,
        Verdict::Never
    );
    // Ranked together, the two slots name one variable of one class.
    let joint = world.head(&["Elt"], &parts, &[0, 0]);
    assert_eq!(world.judge(joint, &[either, either]).0, Verdict::Maybe);
    assert_eq!(world.judge(joint, &[number, number]).0, Verdict::Always);
}

/// `FOR ALL #[Elt] #(FEED x :Elt TO f :<slot>)`: a later slot reads `Elt` at its least instance
/// over `Elt`'s interval — its lower end at a covariant position, its upper end at a contravariant
/// one.
#[test]
fn a_later_slot_reads_an_earlier_variable_at_its_least_instance() {
    let bump = Bump::new();
    let world = World::new(&bump);
    let feed = |slot| {
        world.head(
            &["Elt"],
            &[Kw("FEED"), Slot(world.var(0)), Kw("TO"), Slot(slot)],
            &[],
        )
    };
    let under_number = within(KType::NUMBER.raw());
    let point = Interval::point(KType::NUMBER.raw());
    let handler = feed(world.handler(world.var(0)));
    let numbers = within(world.handler(KType::NUMBER.raw()));
    assert_eq!(
        world.judge(handler, &[under_number, numbers]).0,
        Verdict::Always
    );
    assert_eq!(world.judge(handler, &[point, numbers]).0, Verdict::Always);
    let list = feed(world.types.list(world.var(0)));
    let list_of_number = within(world.types.list(KType::NUMBER.raw()));
    assert_eq!(
        world.judge(list, &[under_number, list_of_number]).0,
        Verdict::Maybe
    );
    assert_eq!(
        world.judge(list, &[point, list_of_number]).0,
        Verdict::Always
    );
}

/// A slot that is a lexical variable admits only what lies under it.
#[test]
fn a_lexical_slot_admits_what_lies_under_it() {
    let bump = Bump::new();
    let world = World::new(&bump);
    let elt = world
        .types
        .lexical(0, world.name("Elt"), KType::NUMBER)
        .raw();
    let inner = world.head(&[], &[Kw("INNER"), Slot(elt)], &[]);
    assert_eq!(
        world.judge(inner, &[within(elt)]),
        (Verdict::Always, Some(vec![]))
    );
    assert_eq!(
        world
            .judge(inner, &[Interval::point(KType::NUMBER.raw())])
            .0,
        Verdict::Maybe
    );
}

/// A slot, read at its greatest instance, that does not lie above its argument's lower end admits
/// no call: the carried type lies above that end. The lower end is read below its rigid variables.
#[test]
fn a_slot_above_no_type_an_argument_can_carry_is_never() {
    let bump = Bump::new();
    let world = World::new(&bump);
    let field =
        |text: &str| BinderSymbol::declared(text, &world.symbols).expect("a bindable token");
    let record = |fields: &[(BinderSymbol, Handle)]| world.types.record(world.region, fields);
    let (numbers, anys) = (
        world.types.list(KType::NUMBER.raw()),
        world.types.list(Handle::ANY),
    );

    let which = world.head(&[], &[Kw("WHICH"), Slot(numbers)], &[]);
    let verdict = |argument: Interval<Handle>| world.judge(which, &[argument]).0;
    assert_eq!(verdict(Interval::point(anys)), Verdict::Never);
    assert_eq!(verdict(within(anys)), Verdict::Maybe);
    assert_eq!(verdict(Interval::point(numbers)), Verdict::Always);

    let elements = world.types.list(world.var(0));
    let pair = world.head(
        &["Elt"],
        &[Kw("PAIR"), Slot(elements), Kw("WITH"), Slot(elements)],
        &[],
    );
    for first in [Interval::point(numbers), within(numbers)] {
        assert_eq!(
            world.judge(pair, &[first, Interval::point(anys)]).0,
            Verdict::Never,
            "`y` is read at `Elt`'s greatest instance, `LIST OF Number`"
        );
    }

    let (a, b) = (field("a"), field("b"));
    let elt = world
        .types
        .lexical(0, world.name("Elt"), KType::NUMBER)
        .raw();
    let strs = world.head(
        &[],
        &[Kw("STRS"), Slot(world.types.list(KType::STR.raw()))],
        &[],
    );
    let needs_b = world.head(
        &[],
        &[Kw("GET"), Slot(record(&[(b, KType::NUMBER.raw())]))],
        &[],
    );
    let has_a = world.head(
        &[],
        &[Kw("GET"), Slot(record(&[(a, KType::NUMBER.raw())]))],
        &[],
    );
    assert_eq!(
        world
            .judge(strs, &[Interval::point(world.types.list(elt))])
            .0,
        Verdict::Maybe,
        "the lower end read below `Elt` is `LIST OF Never`"
    );
    assert_eq!(
        world
            .judge(needs_b, &[Interval::point(record(&[(a, elt)]))])
            .0,
        Verdict::Never
    );
    let bounded_below = Interval {
        lower: record(&[(a, Handle::NEVER)]),
        upper: record(&[(a, Handle::ANY)]),
    };
    assert_eq!(world.judge(needs_b, &[bounded_below]).0, Verdict::Never);
    assert_eq!(world.judge(has_a, &[bounded_below]).0, Verdict::Maybe);
}
