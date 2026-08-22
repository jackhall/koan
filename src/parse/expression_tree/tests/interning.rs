//! The parse side of the run's label table.
//!
//! A bucket key holds keyword symbols and nothing else, so the text a diagnostic renders has to
//! come from somewhere: parse classification is the site that records it. Every keyword token a
//! parse classifies is interned as it is minted.

use super::super::parse;
use crate::machine::core::program_storage;
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

/// Interning is the keyword classifier's job alone: a value token the same parse walks past leaves
/// no entry, so the table's size tracks the keywords a program spells and not its identifiers.
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
        "only `IF` and `THEN` are keyword-class here"
    );
}
