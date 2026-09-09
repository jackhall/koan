//! The operator probe a parse computes, and its agreement with what a group registers under.
//!
//! A chain node carries a probe key in its structural cache: the run digest of the operator
//! symbols it names. A `GROUP` registers its members' whole powerset under keys minted through the
//! same constructor, so a live chain finds its group by construction — neither side touches text,
//! and neither side can drift from the other by spelling.

use super::super::parse;
use crate::machine::core::bindings::powerset_probes;
use crate::machine::core::program_storage;
use crate::machine::model::ast::ExpressionPart;
use crate::machine::model::{KeywordSymbol, LabelInterner};

/// The symbols of a set of glyphs, interned so a probe built from them renders.
fn members(glyphs: &[&str], labels: &LabelInterner) -> Vec<KeywordSymbol> {
    glyphs
        .iter()
        .map(|glyph| {
            KeywordSymbol::declared(glyph, labels).expect("a fixture glyph is keyword-class")
        })
        .collect()
}

/// A chain's probe is the digest over the operators it names — order-free and repeat-free, so
/// `a + b * c` and `a * b + c` probe alike and a longer run of one operator probes as its
/// singleton set.
#[test]
fn a_chain_probes_the_run_of_the_operators_it_names() {
    let program = program_storage();
    let labels = LabelInterner::new();
    let probe = |source: &str| {
        parse(program.brand(), &labels, source).expect("parse should succeed")[0].operator_probe()
    };
    assert_eq!(
        probe("a + b * c"),
        Some(KeywordSymbol::of_run(&members(&["+", "*"], &labels))),
    );
    assert_eq!(probe("a + b * c"), probe("a * b + c"));
    assert_eq!(
        probe("a + b + c + d"),
        Some(KeywordSymbol::of_run(&members(&["+"], &labels))),
    );
}

/// The registration side and the probe side meet: every key `powerset_probes` installs for a
/// member set is a key some chain over those members mints, and the chain naming the whole set
/// probes the full-set key.
#[test]
fn a_chains_probe_is_a_key_its_group_registers_under() {
    let program = program_storage();
    let labels = LabelInterner::new();
    let installed = powerset_probes(&members(&["+", "*"], &labels), &labels);
    for (source, glyphs) in [
        ("a + b * c", &["+", "*"][..]),
        ("a + b + c", &["+"][..]),
        ("a * b * c", &["*"][..]),
    ] {
        let probe = parse(program.brand(), &labels, source).expect("parse should succeed")[0]
            .operator_probe()
            .expect("an operator chain carries a probe");
        assert!(
            installed.contains(&probe),
            "a group over `+ *` registers the key {source:?} probes ({glyphs:?})",
        );
    }
}

/// The redundant-wrapper peel rewrites a nested node's innards but keeps its part-kind sequence
/// and every keyword symbol of its run, so the survivor carries the cache it was built with rather
/// than re-minting one. The nested chain here is rebuilt twice over by the peel — through the
/// wrapper collapse and again as a part of the outer node — and still probes as its own run.
#[test]
fn a_peeled_nested_chain_carries_its_probe() {
    let program = program_storage();
    let labels = LabelInterner::new();
    let statements = parse(program.brand(), &labels, "foo ((a + b * c))").expect("parse");
    let [_head, nested] = statements[0].parts else {
        panic!("the peel leaves an identifier and one nested chain");
    };
    let ExpressionPart::Expression(chain) = nested.value else {
        panic!("the second part is the nested chain");
    };
    assert_eq!(
        chain.operator_probe(),
        Some(KeywordSymbol::of_run(&members(&["+", "*"], &labels))),
    );
}
