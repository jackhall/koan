//! Layout tests: what a body's value-binder run holds, in what order, and what the merge with a
//! signature's parameters does to it.

use super::SlotLayout;
use crate::machine::model::KType;
use crate::memory::{ProgramBrand, program_storage};
use crate::parse::KExpression;
use crate::parse::labels::BinderSymbol;
use crate::parse::parse;
use crate::parse::{LabelInterner, TypeSymbol, ValueSymbol};

/// The lone statement `src` parses to — the body a layout is read off.
fn body<'a>(brand: ProgramBrand<'a>, src: &str) -> KExpression<'a> {
    parse(brand, &LabelInterner::new(), src)
        .expect("parse")
        .into_iter()
        .next()
        .expect("one statement")
}

fn value(text: &str) -> ValueSymbol {
    ValueSymbol::classify(text).expect("a value token")
}

/// Every `(name, position)` of a layout, in slot order.
fn pairs(layout: &SlotLayout<'_>) -> Vec<(ValueSymbol, usize)> {
    layout.iter().map(|(_, name, at)| (name, at)).collect()
}

/// A statement block's value binders land at their statements' own positions (`i + 1`), sorted by
/// symbol — so slot order is symbol order and the position rides along unsorted.
#[test]
fn block_binders_carry_statement_positions() {
    let program = program_storage();
    let brand = program.brand();
    let block = body(brand, "((LET c = 1) (LET a = 2) (LET b = 3))");
    let layout = SlotLayout::of_body(brand.region(), &block);

    let mut expected = vec![(value("c"), 1), (value("a"), 2), (value("b"), 3)];
    expected.sort();
    assert_eq!(pairs(layout), expected);
    // Slot order is the sorted order, and every name resolves back to its own slot.
    for (slot, name, at) in layout.iter() {
        assert_eq!(layout.slot_of(name), Some(slot));
        assert_eq!(layout.position(slot), at);
        assert_eq!(layout.name(slot), name);
    }
}

/// A single-statement body is its own statement, at position 1.
#[test]
fn single_statement_body_binds_at_one() {
    let program = program_storage();
    let brand = program.brand();
    let single = body(brand, "(LET x = 1)");
    let layout = SlotLayout::of_body(brand.region(), &single);
    assert_eq!(pairs(layout), vec![(value("x"), 1)]);
}

/// A redundant paren wrapper reads through to the statement it wraps — the same `statement_spine`
/// read the claim stamp takes.
#[test]
fn paren_wrapper_reads_through() {
    let program = program_storage();
    let brand = program.brand();
    let wrapped = body(brand, "((LET x = 1))");
    assert_eq!(
        pairs(SlotLayout::of_body(brand.region(), &wrapped)),
        vec![(value("x"), 1)]
    );
}

/// A type binder takes no slot: it lands in the keyed `types` channel, which slotting leaves alone.
/// Neither does a statement that binds nothing.
#[test]
fn type_binders_and_non_binders_take_no_slot() {
    let program = program_storage();
    let brand = program.brand();
    let block = body(brand, "((NEWTYPE Meters = Int) (LET x = 1) (x))");
    assert_eq!(
        pairs(SlotLayout::of_body(brand.region(), &block)),
        vec![(value("x"), 2)]
    );
}

/// A body binding no value takes the shared empty layout and bumps nothing.
#[test]
fn binderless_body_is_empty() {
    let program = program_storage();
    let brand = program.brand();
    let plain = body(brand, "(a b)");
    let layout = SlotLayout::of_body(brand.region(), &plain);
    assert!(layout.is_empty());
    assert!(std::ptr::eq(layout, SlotLayout::EMPTY));
}

/// A name bound twice keeps its first (lowest) position — the entry a bind-once table would hold,
/// so the second binder rebinds that slot rather than opening another.
#[test]
fn repeated_name_keeps_the_earliest_position() {
    let program = program_storage();
    let brand = program.brand();
    let block = body(brand, "((LET x = 1) (LET x = 2))");
    assert_eq!(
        pairs(SlotLayout::of_body(brand.region(), &block)),
        vec![(value("x"), 1)]
    );
}

/// The merge: value parameters sit at position 0, type-denoting parameters take no slot, and a
/// parameter beats a body `LET` of the same name on position.
#[test]
fn merge_places_parameters_at_zero() {
    let program = program_storage();
    let brand = program.brand();
    let block = body(brand, "((LET zz = 1) (LET p = 2))");
    let inner = SlotLayout::of_body(brand.region(), &block);
    let params = [
        (BinderSymbol::Value(value("p")), KType::ANY),
        (
            BinderSymbol::Type(TypeSymbol::classify("Elt").expect("a type token")),
            KType::ANY,
        ),
        (BinderSymbol::Value(value("q")), KType::ANY),
    ];
    let merged = SlotLayout::for_function(brand.region(), &params, inner);

    let mut expected = vec![(value("p"), 0), (value("q"), 0), (value("zz"), 1)];
    expected.sort();
    assert_eq!(pairs(merged), expected);
}

/// A re-homed layout is an independent copy with the same content — what a copied environment's
/// scope takes at the destination region.
#[test]
fn rehomed_layout_copies_content() {
    let program = program_storage();
    let brand = program.brand();
    let block = body(brand, "((LET a = 1) (LET b = 2))");
    let source = SlotLayout::of_body(brand.region(), &block);

    let other = program_storage();
    let copy = source.rehomed(other.brand().region());
    assert_eq!(pairs(copy), pairs(source));
    assert!(!std::ptr::eq(source, copy));
}
