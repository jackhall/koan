//! The parse side of the run's label table.
//!
//! A bucket key holds keyword symbols and nothing else, and a `Type` part holds only its token's
//! symbol, so the text a diagnostic renders has to come from somewhere: parse classification is the
//! site that records it. Every keyword and Type token a parse classifies is interned as it is
//! minted.

use super::super::parse;
use crate::machine::core::program_storage;
use crate::machine::model::ast::ExpressionPart;
use crate::machine::model::{KeywordSymbol, LabelInterner};

/// Both keyword spellings — alphabetic tokens and pure-symbol operator glyphs — resolve back out
/// of the interner the parse was handed.
#[test]
fn a_parse_interns_every_keyword_token_it_classifies() {
    for (source, keywords) in [
        ("IF x THEN y ELSE z", &["IF", "THEN", "ELSE"][..]),
        ("a + b * c", &["+", "*"][..]),
    ] {
        let program = program_storage();
        let labels = LabelInterner::new();
        parse(program.brand(), &labels, source).expect("parse should succeed");
        for keyword in keywords {
            let symbol = KeywordSymbol::of(keyword)
                .expect("a fixture keyword is keyword-class")
                .symbol();
            assert_eq!(
                labels.resolve(symbol).as_deref(),
                Some(*keyword),
                "parsing {source:?} should record {keyword:?}",
            );
        }
    }
}

/// A `Type` token's part carries its symbol and nothing else, so the parse is the declaration seam
/// that records the spelling — the one reading every later diagnostic resolves back.
#[test]
fn a_type_token_part_carries_its_symbol_and_the_parse_records_the_spelling() {
    let program = program_storage();
    let labels = LabelInterner::new();
    let exprs = parse(program.brand(), &labels, "Foo").expect("parse should succeed");
    let [statement] = exprs.as_slice() else {
        panic!("one statement");
    };
    let ExpressionPart::Type(name) = statement.parts[0].value else {
        panic!("a Type token parses to a `Type` part");
    };
    assert_eq!(labels.resolve(name.symbol()).as_deref(), Some("Foo"));
}

/// Interning is the keyword and Type classifiers' job: a value token the same parse walks past
/// leaves no entry, so the table's size tracks the names a program can name a binding by and not
/// its identifiers.
#[test]
fn a_parse_records_no_entry_for_a_value_token() {
    let program = program_storage();
    let labels = LabelInterner::new();
    parse(program.brand(), &labels, "IF flag THEN flag").expect("parse should succeed");
    assert_eq!(
        labels.resolve(crate::machine::model::labels::Symbol::of("flag")),
        None
    );
    assert_eq!(
        labels.len(),
        2,
        "only `IF` and `THEN` classify as anything the parse records here"
    );
}
