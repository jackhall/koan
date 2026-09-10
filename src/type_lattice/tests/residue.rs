//! What a law cannot express.
//!
//! Each test here says which law would have covered it and why it cannot: a panic that has no
//! algebraic statement, a byte layout whose whole point is that it never moves, or a worked example
//! that pins the *edge* of a law rather than the law itself.

use crate::parse::{BinderSymbol, LabelInterner, TypeSymbol};

use crate::type_lattice::digest::empty_schema_digest;
use crate::type_lattice::handle::KType;
use crate::type_lattice::lattice::{join, meet};
use crate::type_lattice::node::TypeNode;
use crate::type_lattice::order::is_subtype_of;
use crate::type_lattice::record::Record;
use crate::type_lattice::registry::TypeRegistry;
use crate::type_lattice::shape::DispatchTokenElement;
use crate::type_lattice::unify::{Collector, UnifyFailure, admits_with};
use crate::type_lattice::walk::Variance;

/// No law: a handle that names no interned node is a bug in whoever minted it, not a value the
/// algebra relates. The panic is the contract.
#[test]
#[should_panic(expected = "names no interned node")]
fn reading_an_uninterned_handle_panics() {
    let types = TypeRegistry::new();
    let stranger = KType::from_digest(crate::type_lattice::digest::TypeDigest(0xdead_beef));
    types.with_node(stranger, |_| ());
}

/// No law: `union_of` of nothing has no operand for a property to quantify over. The empty union is
/// the bottom, which is what makes `Never` the join's identity.
#[test]
fn a_union_of_nothing_is_never() {
    let types = TypeRegistry::new();
    assert_eq!(types.union_of(&[]), KType::NEVER);
    assert_eq!(types.union_of(&[KType::NEVER, KType::NEVER]), KType::NEVER);
}

/// No law: the empty schema's byte layout is a fixed point of the recipe, and the whole value of
/// pinning it is that no property may derive it. `:Module` and a user's zero-member `SIG E = ()`
/// share one content identity.
#[test]
fn the_empty_schema_digest_is_the_module_top() {
    let types = TypeRegistry::new();
    let empty = types.signature(crate::type_lattice::schema::SigSchema::empty());
    assert_eq!(empty, KType::EMPTY_SIGNATURE);
    types.with_node(empty, |node| match node {
        TypeNode::Signature { schema_digest, .. } => {
            assert_eq!(*schema_digest, empty_schema_digest());
        }
        _ => panic!("the signature door interned something else"),
    });
}

/// The **edge** of the solving law, not the law: the two worked examples the design states, which
/// fix which side of "no maximum" each falls on.
#[test]
fn a_twice_used_variable_takes_the_maximum_or_fails() {
    let labels = LabelInterner::new();
    let types = TypeRegistry::new();
    let keyword = crate::parse::KeywordSymbol::declared("PURE", &labels).expect("a keyword token");
    let element = types.quantified(0, KType::ANY);
    let name = TypeSymbol::declared("Elt", &labels).expect("a Type token");
    let shape = types
        .shape_type(
            &[name],
            &[
                DispatchTokenElement::Keyword(keyword),
                DispatchTokenElement::Slot(element),
                DispatchTokenElement::Slot(element),
            ],
            KType::NULL,
        )
        .handle;
    let slots = crate::type_lattice::schema::shape_slots(shape, &types);
    let solve = |arguments: [KType; 2]| {
        let mut collector = Collector::new(1);
        for (slot, argument) in slots.iter().zip(arguments.iter()) {
            admits_with(&types, *slot, *argument, Variance::Co, &mut collector)?;
        }
        collector.solve(&types)
    };
    let mixed = types.union_of(&[KType::NUMBER, KType::STR]);
    // Two arguments of one type solve to that type; an argument and a union it lies inside solve to
    // the union, because the union *is* the maximum of the two contributions.
    assert_eq!(
        solve([KType::NUMBER, KType::NUMBER]),
        Ok(vec![KType::NUMBER])
    );
    assert_eq!(solve([KType::NUMBER, mixed]), Ok(vec![mixed]));
    // Two unrelated arguments have no maximum, and the solver refuses to mint their union.
    assert!(matches!(
        solve([KType::NUMBER, KType::STR]),
        Err(UnifyFailure::NoMaximum { index: 0, .. })
    ));
}

/// The **edge** of the canonical-form law: which replacement a single occurrence takes is decided
/// by its polarity, and the law that canonical form is a fixed point cannot say which of the two it
/// settled on.
#[test]
fn a_single_occurrence_takes_its_bound_or_never() {
    let labels = LabelInterner::new();
    let types = TypeRegistry::new();
    let keyword = crate::parse::KeywordSymbol::declared("PURE", &labels).expect("a keyword token");
    let name = TypeSymbol::declared("Elt", &labels).expect("a Type token");
    let variable = types.quantified(0, KType::NUMBER);
    let head = |slot: KType, ret: KType| {
        types
            .shape_type(
                &[name],
                &[
                    DispatchTokenElement::Keyword(keyword),
                    DispatchTokenElement::Slot(slot),
                ],
                ret,
            )
            .handle
    };
    // Contravariant and alone: the caller picks it, so the slot accepts anything under the bound.
    let in_slot = head(variable, KType::NULL);
    assert_eq!(in_slot, head(KType::NUMBER, KType::NULL));
    // Covariant and alone: it must hold at every instantiation, which only the bottom does.
    let in_return = head(KType::STR, variable);
    assert_eq!(in_return, head(KType::STR, KType::NEVER));
}

/// No law: `Record`'s order-blind equality and its hash agreeing is a property of the container,
/// and the generated types never build two records differing only in field order.
#[test]
fn a_record_is_order_blind_in_identity_and_ordered_in_presentation() {
    let labels = LabelInterner::new();
    let types = TypeRegistry::new();
    let x = BinderSymbol::declared("x", &labels).expect("a bindable token");
    let y = BinderSymbol::declared("y", &labels).expect("a bindable token");
    let forwards = Record::from_pairs([(x, KType::NUMBER), (y, KType::STR)]);
    let backwards = Record::from_pairs([(y, KType::STR), (x, KType::NUMBER)]);
    assert_eq!(forwards, backwards);
    assert_eq!(types.record(forwards.clone()), types.record(backwards));
    assert_eq!(
        forwards.keys().collect::<Vec<_>>(),
        vec![x, y],
        "declaration order survives for rendering",
    );
}

/// The **edge** of the width laws: which side of a width verdict each arm sits on has no algebraic
/// statement beyond the order itself, so the four arms are pinned by example.
#[test]
fn width_runs_the_way_each_arm_declares() {
    let labels = LabelInterner::new();
    let types = TypeRegistry::new();
    let x = BinderSymbol::declared("x", &labels).expect("a bindable token");
    let y = BinderSymbol::declared("y", &labels).expect("a bindable token");
    let wide = types.record(Record::from_pairs([(x, KType::NUMBER), (y, KType::STR)]));
    let narrow = types.record(Record::from_pairs([(x, KType::NUMBER)]));
    assert!(
        is_subtype_of(&types, wide, narrow),
        "records are width-superset"
    );
    assert!(!is_subtype_of(&types, narrow, wide));
    assert_eq!(meet(&types, wide, narrow), wide);
    assert_eq!(join(&types, wide, narrow), narrow);

    let few = types.function_type(Record::from_pairs([(x, KType::NUMBER)]), KType::NULL);
    let many = types.function_type(
        Record::from_pairs([(x, KType::NUMBER), (y, KType::STR)]),
        KType::NULL,
    );
    assert!(
        is_subtype_of(&types, few, many),
        "a function subtype asks for no name the supertype does not",
    );
    assert!(!is_subtype_of(&types, many, few));
}
