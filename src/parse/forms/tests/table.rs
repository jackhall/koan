//! Form-table shape: the invariants every reader of [`FORMS`] depends on.

use crate::parse::forms::binder::{BinderFacts, BinderSurface};
use crate::parse::forms::{FORMS, Form, FormId, KeyElementSpec, render_key};
use crate::parse::{DispatchShape, KeyElement, PartClass, classify_dispatch_shape};

/// Every form the table gives binder facts, with those facts beside it.
fn binder_forms() -> impl Iterator<Item = (&'static Form, BinderFacts)> {
    FORMS
        .iter()
        .filter_map(|form| form.binder.map(|binder| (form, binder)))
}

/// A spec key as the key a parts run spelling it would store.
fn key_elements(form: &Form) -> Vec<KeyElement> {
    form.key
        .iter()
        .map(|element| match element {
            KeyElementSpec::Keyword(name) => KeyElement::Keyword(name.symbol()),
            KeyElementSpec::Slot => KeyElement::Slot,
        })
        .collect()
}

/// A tag names its own row. `FormId` is declared in table order, so a reader that has a tag can
/// index the table by it, and a row inserted without its tag — or a tag reordered — fails here.
#[test]
fn every_tag_sits_at_its_own_index() {
    for (index, form) in FORMS.iter().enumerate() {
        assert_eq!(
            form.id as usize,
            index,
            "form key {:?} sits at index {index} under tag {:?}",
            render_key(form.key),
            form.id
        );
    }
}

/// Keys are pairwise distinct. `form_for` takes the first match, so two rows spelling one run would
/// make every fact a node caches depend on table order.
#[test]
fn no_two_forms_spell_the_same_key() {
    for (index, form) in FORMS.iter().enumerate() {
        for other in &FORMS[index + 1..] {
            assert_ne!(
                render_key(form.key),
                render_key(other.key),
                "two forms spell the key {:?}",
                render_key(form.key)
            );
        }
    }
}

/// A reserved form registers nothing, so it declares no binder either: a binder's extractors run on
/// a statement that reached a live bucket.
#[test]
fn no_reserved_form_declares_a_binder() {
    for form in FORMS.iter().filter(|form| form.reserved) {
        assert!(
            form.binder.is_none(),
            "reserved form {:?} declares a binder",
            render_key(form.key)
        );
    }
}

/// Every lazy-slot index names a slot position of its own key, in ascending order, and its kind set
/// is non-empty — the stamp is read as `parts[index]`, so a keyword position, an index past the run,
/// or a run out of order would misread the statement.
#[test]
fn every_lazy_slot_names_a_slot_position() {
    for form in FORMS {
        assert!(
            form.lazy_slots.windows(2).all(|pair| pair[0].0 < pair[1].0),
            "form key {:?} lists its slots out of ascending order",
            render_key(form.key)
        );
        for (index, kinds) in form.lazy_slots {
            assert!(
                *index < form.key.len(),
                "form key {:?} declares slot {index} past its run",
                render_key(form.key)
            );
            assert!(
                matches!(form.key[*index], KeyElementSpec::Slot),
                "form key {:?} declares its keyword position {index} lazy",
                render_key(form.key)
            );
            assert!(
                !kinds.is_empty(),
                "form key {:?} declares an empty kind set at slot {index}",
                render_key(form.key)
            );
        }
    }
}

/// The tag vocabulary is exhaustive over the table: `FormId` gains no variant without a row.
#[test]
fn the_table_is_as_long_as_the_tag_vocabulary() {
    assert_eq!(FORMS.len(), FormId::Eval as usize + 1);
}

/// Every masked index names a slot position of its own key — the flip writes `parts[index]`, so a
/// keyword position or an index past the run would corrupt the statement.
#[test]
fn every_masked_index_names_a_slot_position() {
    for (form, binder) in binder_forms() {
        for &index in binder.type_slots {
            assert!(
                index < form.key.len(),
                "form key {:?} masks slot {index} past its run",
                render_key(form.key)
            );
            assert!(
                matches!(form.key[index], KeyElementSpec::Slot),
                "form key {:?} masks its keyword position {index}",
                render_key(form.key)
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
            .key
            .iter()
            .any(|element| matches!(element, KeyElementSpec::Keyword(name) if name.text() == "OP"));
        assert_eq!(
            binder.surface == BinderSurface::OperatorDef,
            names_op,
            "form key {:?} disagrees with its surface marker",
            render_key(form.key),
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
            render_key(form.key)
        );
    }
}
