//! Priority classes: the worked examples the dispatch design states, pinned as the edges of the
//! ranking laws in [`properties`](super::properties).
//!
//! No law says which candidate a particular pair of heads selects, so each selection example here is
//! the design's own, verbatim: `SHOW` decided by its first class, `MOVE` by its second, `PAIR`
//! telling one variable from two, `TAKE` reading an unadmitted class's variable as its bound. The
//! rest pin the normalizer, the ranking's place in a shape's identity and rendering, the verdict
//! table's reuse, and the two signature relations a ranking refuses in.

use crate::memory::{Bump, BumpAllocator};
use crate::symbols::{KeywordSymbol, SymbolInterner, TypeSymbol};

use crate::type_lattice::handle::KType;
use crate::type_lattice::node::TypeNode;
use crate::type_lattice::ranking::{admit_by_class, select_by_class};
use crate::type_lattice::registry::TypeRegistry;
use crate::type_lattice::render::display_name;
use crate::type_lattice::schema::SchemaDraft;
use crate::type_lattice::shape::{DispatchTokenElement, RawRank, Specificity, dense_classes};
use crate::type_lattice::sig_relations::{
    SigSubtypeFailure, meet_schemas, shape_specificity, sig_subtype,
};

/// One position of a test head: a keyword by its text, or a slot's type.
enum Part<'a> {
    Kw(&'a str),
    Slot(KType),
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
    fn var(&self, index: usize) -> KType {
        self.types.quantified(index, KType::ANY)
    }

    fn union(&self, members: &[KType]) -> KType {
        self.types.union_of(self.region, members)
    }

    /// The shape `FOR ALL #[<group>] #(<parts>) -> Any` under `classes`.
    fn head(&self, group: &[&str], parts: &[Part<'_>], classes: &[u8]) -> KType {
        let names: Vec<TypeSymbol> = group.iter().map(|text| self.name(text)).collect();
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
            .shape_type(self.region, &names, &elements, classes, KType::ANY)
            .handle
    }

    /// The survivors of selection over `shapes`.
    fn select(&self, shapes: &[KType]) -> Vec<usize> {
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
    let parts = [Kw("MOVE"), Slot(KType::ANY), Kw("TO"), Slot(KType::ANY)];
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

    let render = |kt| display_name(kt, &world.types, &world.symbols).to_string();
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
    let numbers = world.types.list(KType::NUMBER);
    let mixed = world.types.list(world.union(&[KType::NUMBER, KType::STR]));
    let admits = |shape, arguments: &[KType]| {
        admit_by_class(&world.types, world.region, shape, arguments).is_some()
    };
    assert!(!admits(written, &[numbers, mixed]));
    assert!(admits(written, &[mixed, numbers]));
    assert!(admits(joint, &[numbers, mixed]));
    assert!(admits(joint, &[mixed, numbers]));
    assert_eq!(
        admit_by_class(&world.types, world.region, written, &[mixed, numbers]),
        Some(&[world.union(&[KType::NUMBER, KType::STR])][..])
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
            Slot(KType::NUMBER),
            Kw("WITH"),
            Slot(KType::ANY),
        ],
        &[],
    );
    let generic = world.head(
        &["Elt"],
        &[Kw("SHOW"), Slot(world.var(0)), Kw("WITH"), Slot(KType::STR)],
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
            Slot(world.union(&[KType::STR, KType::BOOL])),
            Kw("TO"),
            Slot(KType::NUMBER),
        ],
        &[],
    );
    let wide = world.head(
        &[],
        &[
            Kw("MOVE"),
            Slot(world.union(&[KType::NUMBER, KType::STR])),
            Kw("TO"),
            Slot(KType::ANY),
        ],
        &[],
    );
    assert_eq!(world.select(&[narrow, wide]), [0]);
    assert_eq!(
        shape_specificity(&world.types, world.region, narrow, wide),
        Specificity::StrictlyMore
    );
}

/// `PAIR 1 WITH 2`: one variable beats two. Canonical form reads the two-variable head as
/// `#(PAIR _ :Any WITH _ :Any)`; the one-variable head's `Elt`, solved from `x`, reads at `y` as an
/// unknown type under that solution, which `Any` does not lie under.
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
    let any = world.head(
        &[],
        &[Kw("PAIR"), Slot(KType::ANY), Kw("WITH"), Slot(KType::ANY)],
        &[],
    );
    assert_eq!(two, any);
    assert_eq!(world.select(&[one, two]), [0]);
    assert_eq!(world.select(&[two, one]), [1]);
    assert_eq!(
        shape_specificity(&world.types, world.region, one, one),
        Specificity::Equal
    );
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
            Slot(world.union(&[KType::STR, world.types.list(KType::NUMBER)])),
            Kw("WITH"),
            Slot(KType::NUMBER),
        ],
        &[],
    );
    let generic = world.head(
        &["Elt"],
        &[
            Kw("TAKE"),
            Slot(world.union(&[KType::NUMBER, world.types.list(world.var(0))])),
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
        &[Kw("MIX"), Slot(KType::NUMBER), Kw("AND"), Slot(KType::ANY)],
        &[0, 0],
    );
    let right = world.head(
        &[],
        &[Kw("MIX"), Slot(KType::ANY), Kw("AND"), Slot(KType::NUMBER)],
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
        &[Kw("MOVE"), Slot(KType::STR), Kw("TO"), Slot(KType::NUMBER)],
        &[],
    );
    let b = world.head(
        &[],
        &[Kw("MOVE"), Slot(KType::ANY), Kw("TO"), Slot(KType::ANY)],
        &[],
    );
    let _ = shape_specificity(&world.types, world.region, a, b);
    let (recorded, _) = world.types.verdict_tally();
    let _ = shape_specificity(&world.types, world.region, a, b);
    assert_eq!(world.types.verdict_tally().0, recorded);
}

/// A signature member ranked `2 … 1` is not satisfied by a module whose bucket is written-order,
/// and two signatures ranking one key two ways have no meet.
#[test]
fn a_ranking_disagreement_refuses_in_the_signature_relations() {
    let bump = Bump::new();
    let world = World::new(&bump);
    let parts = [Kw("MOVE"), Slot(KType::ANY), Kw("TO"), Slot(KType::ANY)];
    let ranked = world.head(&[], &parts, &[1, 0]);
    let written = world.head(&[], &parts, &[]);
    let signature = |member| {
        let mut draft = SchemaDraft::new(world.region);
        draft.keyworded.push(member);
        let kt = world.types.signature(world.region, draft);
        match world.types.node(kt) {
            TypeNode::Signature { schema, .. } => schema,
            _ => unreachable!("the signature door interns a signature"),
        }
    };
    assert!(matches!(
        sig_subtype(
            &world.types,
            world.region,
            signature(written),
            signature(ranked)
        ),
        Err(SigSubtypeFailure::RankingMismatch { .. })
    ));
    assert!(
        sig_subtype(
            &world.types,
            world.region,
            signature(ranked),
            signature(ranked)
        )
        .is_ok()
    );
    assert!(
        meet_schemas(
            &world.types,
            world.region,
            signature(written),
            signature(ranked)
        )
        .is_none()
    );
}
