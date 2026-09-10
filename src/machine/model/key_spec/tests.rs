//! Form-table shape: the invariants every reader of [`FORMS`] depends on.

use super::{FORMS, FormId, KeyElementSpec, render_key};

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

/// Every lazy-slot index names a slot position of its own key, and its kind set is non-empty — the
/// stamp is read as `parts[index]`, so a keyword position or an index past the run would misread
/// the statement.
#[test]
fn every_lazy_slot_names_a_slot_position() {
    for form in FORMS {
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
