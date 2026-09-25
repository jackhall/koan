//! BuiltinShape-table shape: the invariants every reader of [`BUILTIN_SHAPES`] depends on.

use crate::parse::builtin_shapes::binder::{BinderFacts, BinderSurface};
use crate::parse::builtin_shapes::lazy::LazyKinds;
use crate::parse::builtin_shapes::{
    BUILTIN_SHAPES, BuiltinShape, BuiltinShapeId, ShapeElement, render_key,
};
use crate::parse::{DispatchShape, KeyElement, PartClass, classify_dispatch_shape};

/// Every form the table gives binder facts, with those facts beside it.
fn binder_forms() -> impl Iterator<Item = (&'static BuiltinShape, BinderFacts)> {
    BUILTIN_SHAPES
        .iter()
        .filter_map(|form| form.binder.map(|binder| (form, binder)))
}

/// An entry's run erased to the key a parts run spelling it would store.
fn key_elements(form: &BuiltinShape) -> Vec<KeyElement> {
    form.elements
        .iter()
        .map(|element| match element {
            ShapeElement::Keyword(name) => KeyElement::Keyword(name.symbol()),
            ShapeElement::Slot { .. } => KeyElement::Slot,
        })
        .collect()
}

/// Every bucket's raw-capture kinds, written out by hand: one statement of what each entry's slots
/// keep raw that is independent of the derivation, so the two can be compared. An index the run
/// omits keeps nothing raw. This is the pin on `lazy_kinds_at` — a slot retyped in a way that
/// changes what a reader sees fails here rather than silently moving when a group evaluates.
const RECORDED_RAW_SLOTS: &[(BuiltinShapeId, &[(usize, LazyKinds)])] = &[
    (BuiltinShapeId::LetValue, &[]),
    (BuiltinShapeId::TypeDeclaration, &[(1, CODE)]),
    (BuiltinShapeId::Module, &[(3, CODE)]),
    (BuiltinShapeId::GroupFoldLeft, &[(5, CODE)]),
    (BuiltinShapeId::GroupFoldRight, &[(5, CODE)]),
    (
        BuiltinShapeId::GroupPairwiseFoldLeft,
        &[(4, CODE), (7, CODE)],
    ),
    (
        BuiltinShapeId::GroupPairwiseFoldRight,
        &[(4, CODE), (7, CODE)],
    ),
    (BuiltinShapeId::Sig, &[(3, CODE)]),
    (BuiltinShapeId::Union, &[(1, CODE), (3, CODE)]),
    (BuiltinShapeId::NewTypeDefinition, &[(3, RAW_TYPE)]),
    (BuiltinShapeId::NewTypeDeclaration, &[(1, CODE)]),
    (BuiltinShapeId::Val, &[]),
    (BuiltinShapeId::Lambda, &[(3, RAW_TYPE), (5, CODE)]),
    (BuiltinShapeId::LambdaType, &[]),
    (
        BuiltinShapeId::CombinedLambda,
        &[(4, CODE), (6, RAW_TYPE), (8, CODE)],
    ),
    (
        BuiltinShapeId::QuantifiedLambda,
        &[(3, CODE), (4, RECORD_TYPE), (6, RAW_TYPE), (8, CODE)],
    ),
    (
        BuiltinShapeId::QuantifiedLambdaType,
        &[(3, CODE), (4, RECORD_TYPE), (6, RAW_TYPE)],
    ),
    (
        BuiltinShapeId::CombinedQuantifiedLambda,
        &[(6, CODE), (7, RECORD_TYPE), (9, RAW_TYPE), (11, CODE)],
    ),
    (
        BuiltinShapeId::ExpressionDefinition,
        &[(1, CODE), (3, RAW_TYPE), (5, CODE)],
    ),
    (BuiltinShapeId::ExpressionHead, &[(1, CODE)]),
    (
        BuiltinShapeId::QuantifiedExpressionDefinition,
        &[(3, CODE), (4, CODE), (6, RAW_TYPE), (8, CODE)],
    ),
    (
        BuiltinShapeId::QuantifiedExpressionHead,
        &[(3, CODE), (4, CODE), (6, RAW_TYPE)],
    ),
    (
        BuiltinShapeId::CombinedExpression,
        &[(5, CODE), (7, RAW_TYPE), (9, CODE)],
    ),
    (
        BuiltinShapeId::CombinedQuantifiedExpression,
        &[(7, CODE), (8, CODE), (10, RAW_TYPE), (12, CODE)],
    ),
    (
        BuiltinShapeId::OperatorDefinition,
        &[(1, CODE), (3, RAW_TYPE), (5, CODE)],
    ),
    (
        BuiltinShapeId::OperatorDefinitionReturning,
        &[(1, CODE), (3, RAW_TYPE), (5, RAW_TYPE), (7, CODE)],
    ),
    (
        BuiltinShapeId::UnaryOperatorDefinition,
        &[(2, CODE), (4, RAW_TYPE), (6, CODE)],
    ),
    (
        BuiltinShapeId::UnaryOperatorDefinitionReturning,
        &[(2, CODE), (4, RAW_TYPE), (6, RAW_TYPE), (8, CODE)],
    ),
    (BuiltinShapeId::OperatorHead, &[(1, CODE)]),
    (BuiltinShapeId::OperatorHeadReturning, &[(1, CODE)]),
    (BuiltinShapeId::UnaryOperatorHead, &[]),
    (BuiltinShapeId::UnaryOperatorHeadReturning, &[(2, CODE)]),
    (
        BuiltinShapeId::CombinedOperator,
        &[(4, CODE), (6, RAW_TYPE), (8, CODE)],
    ),
    (
        BuiltinShapeId::CombinedOperatorReturning,
        &[(4, CODE), (6, RAW_TYPE), (8, RAW_TYPE), (10, CODE)],
    ),
    (
        BuiltinShapeId::CombinedUnaryOperator,
        &[(5, CODE), (7, RAW_TYPE), (9, CODE)],
    ),
    (
        BuiltinShapeId::CombinedUnaryOperatorReturning,
        &[(5, CODE), (7, RAW_TYPE), (9, RAW_TYPE), (11, CODE)],
    ),
    (BuiltinShapeId::GroupHeadFoldLeft, &[(4, CODE)]),
    (BuiltinShapeId::GroupHeadFoldRight, &[(4, CODE)]),
    (
        BuiltinShapeId::GroupHeadPairwiseFoldLeft,
        &[(3, CODE), (6, CODE)],
    ),
    (
        BuiltinShapeId::GroupHeadPairwiseFoldRight,
        &[(3, CODE), (6, CODE)],
    ),
    (BuiltinShapeId::Match, &[(5, CODE)]),
    (BuiltinShapeId::MatchOver, &[(7, CODE)]),
    (BuiltinShapeId::Try, &[(1, CODE), (5, CODE)]),
    (BuiltinShapeId::Catch, &[(1, CODE)]),
    (BuiltinShapeId::UsingScope, &[(3, CODE)]),
    (BuiltinShapeId::AscribeOpaque, &[]),
    (BuiltinShapeId::AscribeTransparent, &[]),
    (BuiltinShapeId::CloseOver, &[(2, CODE), (3, CODE)]),
    (BuiltinShapeId::Close, &[(1, CODE)]),
    (BuiltinShapeId::Projection, &[(0, CODE)]),
    (BuiltinShapeId::Attribute, &[]),
    (BuiltinShapeId::Eval, &[]),
];

const CODE: LazyKinds = LazyKinds::CODE;
const RECORD_TYPE: LazyKinds = LazyKinds::RECORD_TYPE;
/// What a type-position slot captures raw: a `:(…)` type expression or a `:{…}` record type.
const RAW_TYPE: LazyKinds = LazyKinds::TYPE_EXPR.with(LazyKinds::RECORD_TYPE);

/// The derivation answers the recorded column, at every index of every run — including the indices
/// the column omits, which keep nothing raw, and one index past the run, which a reader asking
/// about a part that is not there must survive.
#[test]
fn the_derived_raw_slots_are_the_recorded_ones() {
    assert_eq!(RECORDED_RAW_SLOTS.len(), BUILTIN_SHAPES.len());
    for (form, (id, recorded)) in BUILTIN_SHAPES.iter().zip(RECORDED_RAW_SLOTS) {
        assert_eq!(
            form.id, *id,
            "the pin is out of table order at {:?}",
            form.id
        );
        for index in 0..=form.elements.len() {
            let expected = recorded
                .iter()
                .find(|(slot, _)| *slot == index)
                .map_or(LazyKinds::EMPTY, |(_, kinds)| *kinds);
            assert_eq!(
                form.lazy_kinds_at(index),
                expected,
                "{:?} derives the wrong raw kinds at {index}",
                render_key(form.elements)
            );
        }
    }
}

/// A tag names its own row. `BuiltinShapeId` is declared in table order, so a reader that has a tag can
/// index the table by it, and a row inserted without its tag — or a tag reordered — fails here.
#[test]
fn every_tag_sits_at_its_own_index() {
    for (index, form) in BUILTIN_SHAPES.iter().enumerate() {
        assert_eq!(
            form.id as usize,
            index,
            "form key {:?} sits at index {index} under tag {:?}",
            render_key(form.elements),
            form.id
        );
    }
}

/// Keys are pairwise distinct. `builtin_shape_for` takes the first match, so two rows spelling one run would
/// make every fact a node caches depend on table order.
#[test]
fn no_two_forms_spell_the_same_key() {
    for (index, form) in BUILTIN_SHAPES.iter().enumerate() {
        for other in &BUILTIN_SHAPES[index + 1..] {
            assert_ne!(
                render_key(form.elements),
                render_key(other.elements),
                "two forms spell the key {:?}",
                render_key(form.elements)
            );
        }
    }
}

/// A reserved form registers nothing, so it declares no binder either: a binder's extractors run on
/// a statement that reached a live bucket.
#[test]
fn no_reserved_form_declares_a_binder() {
    for form in BUILTIN_SHAPES.iter().filter(|form| form.reserved) {
        assert!(
            form.binder.is_none(),
            "reserved form {:?} declares a binder",
            render_key(form.elements)
        );
    }
}

/// The tag vocabulary is exhaustive over the table: `BuiltinShapeId` gains no variant without a row.
#[test]
fn the_table_is_as_long_as_the_tag_vocabulary() {
    assert_eq!(BUILTIN_SHAPES.len(), BuiltinShapeId::Eval as usize + 1);
}

/// Every masked index names a slot position of its own key — the flip writes `parts[index]`, so a
/// keyword position or an index past the run would corrupt the statement.
#[test]
fn every_masked_index_names_a_slot_position() {
    for (form, binder) in binder_forms() {
        for &index in binder.type_slots {
            assert!(
                index < form.elements.len(),
                "form key {:?} masks slot {index} past its run",
                render_key(form.elements)
            );
            assert!(
                matches!(form.elements[index], ShapeElement::Slot { .. }),
                "form key {:?} masks its keyword position {index}",
                render_key(form.elements)
            );
        }
    }
}

/// The `OperatorDef` marker agrees with the keys it labels: a binder-bearing form is marked iff its
/// key names the `OP` declarator keyword. The marker is what `GROUP`'s member scan keys on, so a new
/// operator surface that forgets it — or a non-operator form that wrongly carries it — fails here
/// rather than silently changing which body statements a group treats as members.
#[test]
fn operator_def_marker_agrees_with_the_keys_it_labels() {
    for (form, binder) in binder_forms() {
        let names_op = form
            .elements
            .iter()
            .any(|element| matches!(element, ShapeElement::Keyword(name) if name.text() == "OP"));
        assert_eq!(
            binder.surface == BinderSurface::OperatorDef,
            names_op,
            "form key {:?} disagrees with its surface marker",
            render_key(form.elements),
        );
    }
}

/// A binder-bearing key classifies `Keyworded`: an install is reached through the keyworded
/// dispatch lane, never through a fast-lane shape or the operator chain.
#[test]
fn every_binder_form_key_classifies_keyworded() {
    for (form, _) in binder_forms() {
        let key = key_elements(form);
        let head = key.first().map(|element| match element {
            KeyElement::Keyword(symbol) => PartClass::Keyword(*symbol),
            KeyElement::Slot => PartClass::Identifier,
        });
        assert_eq!(
            classify_dispatch_shape(&key, head),
            DispatchShape::Keyworded,
            "form key {:?} does not classify Keyworded",
            render_key(form.elements)
        );
    }
}
