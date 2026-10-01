//! BuiltinShape-table shape: the invariants every reader of [`BUILTIN_SHAPES`] depends on.

use crate::memory::Bump;
use crate::parse::builtin_shapes::binder::{BinderFacts, BinderSurface};
use crate::parse::builtin_shapes::role::Reading;
use crate::parse::builtin_shapes::{
    BUILTIN_SHAPES, BuiltinShape, BuiltinShapeId, ShapeElement, render_key,
};
use crate::parse::{DispatchShape, KeyElement, PartClass, classify_dispatch_shape};
use crate::type_lattice::TypeRegistry;

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

/// Every bucket's readings, written out by hand: how the shape builder reads each slot that is not
/// evaluated — as a written quote, as bare syntax, as a container of quotes, or as a label. One
/// statement independent of [`Role::reading`], so the two can be compared; a slot the run omits is
/// evaluated. A slot moved to another reading fails here rather than silently changing what a
/// program must write there.
const RECORDED_READINGS: &[(BuiltinShapeId, &[(usize, Reading)])] = &[
    (BuiltinShapeId::LetValue, &[(1, B)]),
    (BuiltinShapeId::TypeDeclaration, &[]),
    (BuiltinShapeId::Module, &[(1, B), (3, B)]),
    (BuiltinShapeId::GroupFoldLeft, &[(1, B), (5, B)]),
    (BuiltinShapeId::GroupFoldRight, &[(1, B), (5, B)]),
    (
        BuiltinShapeId::GroupPairwiseFoldLeft,
        &[(1, B), (4, Q), (7, B)],
    ),
    (
        BuiltinShapeId::GroupPairwiseFoldRight,
        &[(1, B), (4, Q), (7, B)],
    ),
    (BuiltinShapeId::Sig, &[(1, B), (3, C)]),
    (BuiltinShapeId::QuantifiedSig, &[(1, B), (4, C), (6, C)]),
    (BuiltinShapeId::Union, &[(1, B), (3, C)]),
    (BuiltinShapeId::NewTypeDefinition, &[(1, B), (3, B)]),
    (BuiltinShapeId::NewTypeDeclaration, &[(1, B)]),
    (BuiltinShapeId::Val, &[(1, B), (2, B)]),
    (BuiltinShapeId::Lambda, &[(3, B), (5, Q)]),
    (BuiltinShapeId::LambdaType, &[(3, B)]),
    (BuiltinShapeId::CombinedLambda, &[]),
    (BuiltinShapeId::QuantifiedLambda, &[(3, C), (6, B), (8, Q)]),
    (BuiltinShapeId::QuantifiedLambdaType, &[(3, C), (6, B)]),
    (BuiltinShapeId::CombinedQuantifiedLambda, &[]),
    (
        BuiltinShapeId::ExpressionDefinition,
        &[(1, Q), (3, B), (5, Q)],
    ),
    (BuiltinShapeId::ExpressionHead, &[(1, Q), (3, B)]),
    (
        BuiltinShapeId::QuantifiedExpressionDefinition,
        &[(3, C), (4, Q), (6, B), (8, Q)],
    ),
    (
        BuiltinShapeId::QuantifiedExpressionHead,
        &[(3, C), (4, Q), (6, B)],
    ),
    (BuiltinShapeId::BucketDeclaration, &[(1, Q)]),
    (
        BuiltinShapeId::CombinedExpression,
        &[(1, B), (5, Q), (7, B), (9, Q)],
    ),
    (
        BuiltinShapeId::CombinedQuantifiedExpression,
        &[(1, B), (7, C), (8, Q), (10, B), (12, Q)],
    ),
    (
        BuiltinShapeId::OperatorDefinition,
        &[(1, Q), (3, B), (5, Q)],
    ),
    (
        BuiltinShapeId::OperatorDefinitionReturning,
        &[(1, Q), (3, B), (5, B), (7, Q)],
    ),
    (BuiltinShapeId::UnaryOperatorDefinition, &[]),
    (
        BuiltinShapeId::UnaryOperatorDefinitionReturning,
        &[(2, Q), (4, B), (6, B), (8, Q)],
    ),
    (BuiltinShapeId::OperatorHead, &[(1, Q), (3, B)]),
    (
        BuiltinShapeId::OperatorHeadReturning,
        &[(1, Q), (3, B), (5, B)],
    ),
    (BuiltinShapeId::UnaryOperatorHead, &[]),
    (
        BuiltinShapeId::UnaryOperatorHeadReturning,
        &[(2, Q), (4, B), (6, B)],
    ),
    (
        BuiltinShapeId::CombinedOperator,
        &[(1, B), (4, Q), (6, B), (8, Q)],
    ),
    (
        BuiltinShapeId::CombinedOperatorReturning,
        &[(1, B), (4, Q), (6, B), (8, B), (10, Q)],
    ),
    (BuiltinShapeId::CombinedUnaryOperator, &[]),
    (
        BuiltinShapeId::CombinedUnaryOperatorReturning,
        &[(1, B), (5, Q), (7, B), (9, B), (11, Q)],
    ),
    (BuiltinShapeId::GroupHeadFoldLeft, &[(4, C)]),
    (BuiltinShapeId::GroupHeadFoldRight, &[(4, C)]),
    (BuiltinShapeId::GroupHeadPairwiseFoldLeft, &[(3, Q), (6, C)]),
    (
        BuiltinShapeId::GroupHeadPairwiseFoldRight,
        &[(3, Q), (6, C)],
    ),
    (BuiltinShapeId::Match, &[(3, B), (5, C)]),
    (BuiltinShapeId::MatchOver, &[(3, B), (5, B), (7, C)]),
    (BuiltinShapeId::Try, &[(1, B), (3, B), (5, C)]),
    (BuiltinShapeId::Catch, &[(1, B)]),
    (BuiltinShapeId::UsingScope, &[(3, B)]),
    (BuiltinShapeId::AscribeOpaque, &[(2, B)]),
    (BuiltinShapeId::AscribeTransparent, &[(2, B)]),
    (BuiltinShapeId::CloseOver, &[]),
    (BuiltinShapeId::Close, &[]),
    (BuiltinShapeId::Projection, &[]),
    (BuiltinShapeId::Attribute, &[(2, L)]),
    (BuiltinShapeId::Eval, &[(3, B)]),
    (BuiltinShapeId::UsingCode, &[]),
];

const Q: Reading = Reading::Quote;
const B: Reading = Reading::Bare;
const C: Reading = Reading::Container;
const L: Reading = Reading::Label;

/// Each slot's role reads it as the recorded column says, and every slot the column omits is
/// evaluated.
#[test]
fn every_slot_is_read_as_recorded() {
    assert_eq!(RECORDED_READINGS.len(), BUILTIN_SHAPES.len());
    for (form, (id, recorded)) in BUILTIN_SHAPES.iter().zip(RECORDED_READINGS) {
        assert_eq!(
            form.id, *id,
            "the pin is out of table order at {:?}",
            form.id
        );
        for (index, element) in form.elements.iter().enumerate() {
            let ShapeElement::Slot { role, .. } = element else {
                assert!(
                    recorded.iter().all(|(slot, _)| *slot != index),
                    "{:?} records a reading at its keyword {index}",
                    form.id
                );
                continue;
            };
            let expected = recorded
                .iter()
                .find(|(slot, _)| *slot == index)
                .map_or(Reading::Evaluated, |(_, reading)| *reading);
            assert_eq!(
                role.reading(),
                expected,
                "{:?} reads its slot {index} the wrong way",
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
    assert_eq!(BUILTIN_SHAPES.len(), BuiltinShapeId::UsingCode as usize + 1);
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

/// Every slot type and return the table states names a node every registry pre-seeds, so a
/// container-typed slot whose handle no registry seeds fails here, not at every program that
/// writes it.
#[test]
fn every_slot_type_names_a_seeded_node() {
    let region = Bump::new();
    let types = TypeRegistry::in_region(&region);
    for shape in BUILTIN_SHAPES {
        for element in shape.elements {
            if let ShapeElement::Slot {
                types: slot_types, ..
            } = element
            {
                for handle in *slot_types {
                    let _ = types.node(*handle);
                }
            }
        }
        for handle in shape.returns {
            let _ = types.node(*handle);
        }
    }
}
