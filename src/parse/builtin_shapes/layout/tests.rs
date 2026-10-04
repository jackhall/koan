//! Layout laws: what a body's value-binder run holds, and in what order.

use proptest::prelude::*;

use super::SlotLayout;
use crate::memory::{ProgramBrand, program_storage};
use crate::parse::KExpression;
use crate::parse::parse;
use crate::symbols::{SymbolInterner, ValueSymbol};

/// One statement of a generated body: a value binder, a type binder, or a statement that binds
/// nothing.
#[derive(Clone, Debug)]
enum Statement {
    Bind(String),
    TypeBind(String),
    Plain(String),
}

impl Statement {
    fn source(&self) -> String {
        match self {
            Statement::Bind(name) => format!("(LET {name} = 1)"),
            Statement::TypeBind(name) => format!("(NEWTYPE {name} = Int)"),
            Statement::Plain(name) => format!("({name} {name})"),
        }
    }

    /// The value name this statement binds, if it binds one.
    fn binds(&self) -> Option<&str> {
        match self {
            Statement::Bind(name) => Some(name),
            Statement::TypeBind(_) | Statement::Plain(_) => None,
        }
    }
}

fn statement() -> impl Strategy<Value = Statement> {
    prop_oneof![
        3 => "[a-z]{2,3}".prop_map(Statement::Bind),
        1 => "[A-Z][a-z]{1,3}".prop_map(Statement::TypeBind),
        1 => "[a-z]{2,3}".prop_map(Statement::Plain),
    ]
}

fn value(text: &str) -> ValueSymbol {
    ValueSymbol::classify(text).expect("a value token")
}

/// The lone statement `source` parses to — the body a layout is read off.
fn body<'a>(brand: ProgramBrand<'a>, source: &str) -> KExpression<'a> {
    parse(brand, &SymbolInterner::new(), source)
        .expect("the generated body parses")
        .into_iter()
        .next()
        .expect("one statement")
}

/// Every `(name, position)` of a layout, in slot order.
fn pairs(layout: &SlotLayout<'_>) -> Vec<(ValueSymbol, usize)> {
    layout.iter().map(|(_, name, at)| (name, at)).collect()
}

/// The distinct entries of `entries`, keeping the lowest position per name, in symbol order — the
/// run a bind-once table would hold.
fn expected(mut entries: Vec<(ValueSymbol, usize)>) -> Vec<(ValueSymbol, usize)> {
    entries.sort();
    entries.dedup_by_key(|(name, _)| *name);
    entries
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: crate::tests::case_share(1, 4),
        ..ProptestConfig::default()
    })]

    /// A body's slots are its distinct value binders in symbol order, each at the position its own
    /// statement submits at (`i + 1`), with a repeated name keeping its earliest. Type binders and
    /// non-binders take no slot: they land in the keyed channels, which slotting does not touch.
    /// The accessors are mutually inverse — a name resolves to the slot that names it back.
    #[test]
    fn a_layout_is_the_distinct_value_binders_in_symbol_order(
        statements in prop::collection::vec(statement(), 1..6),
    ) {
        let program = program_storage();
        let brand = program.brand();
        let source: String = statements
            .iter()
            .map(Statement::source)
            .collect::<Vec<_>>()
            .join(" ");
        let layout = SlotLayout::of_body(brand.writer(), &body(brand, &source));

        let slots = expected(
            statements
                .iter()
                .enumerate()
                .filter_map(|(index, statement)| {
                    statement.binds().map(|name| (value(name), index + 1))
                })
                .collect(),
        );
        prop_assert_eq!(pairs(layout), slots.clone());
        prop_assert_eq!(layout.len(), slots.len());
        prop_assert_eq!(layout.is_empty(), slots.is_empty());
        for (slot, name, at) in layout.iter() {
            prop_assert_eq!(layout.slot_of(name), Some(slot));
            prop_assert_eq!(layout.position(slot), at);
            prop_assert_eq!(layout.name(slot), name);
        }
    }

    /// Redundant paren wrapping is transparent: a layout reads through to the statement it wraps,
    /// the same `statement_spine` read the claim stamp takes.
    #[test]
    fn redundant_paren_wrapping_is_transparent(name in "[a-z]{2,3}") {
        let program = program_storage();
        let brand = program.brand();
        let bare = SlotLayout::of_body(brand.writer(), &body(brand, &format!("(LET {name} = 1)")));
        prop_assert_eq!(pairs(bare), vec![(value(&name), 1)]);
        for depth in 1..3 {
            let source = format!(
                "{}(LET {name} = 1){}",
                "(".repeat(depth),
                ")".repeat(depth),
            );
            let wrapped = SlotLayout::of_body(brand.writer(), &body(brand, &source));
            prop_assert_eq!(pairs(wrapped), pairs(bare), "{}", source);
        }
    }
}

/// A body binding no value takes the shared empty layout and writes nothing.
#[test]
fn binderless_body_is_empty() {
    let program = program_storage();
    let brand = program.brand();
    let plain = body(brand, "(a b)");
    let layout = SlotLayout::of_body(brand.writer(), &plain);
    assert!(layout.is_empty());
    assert!(std::ptr::eq(layout, SlotLayout::EMPTY));
}
