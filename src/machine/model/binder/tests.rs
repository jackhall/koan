//! Binder-model tests: the spec⟺registration consistency pin (the table matches the live builtin
//! function table) and the parse-time binder plan each statement caches.

use std::collections::HashSet;

use super::{BINDER_SPECS, BinderSpec, StoredBinderKey};
use crate::builtins::test_support::kw_part;
use crate::machine::core::{ProgramBrand, RegionBrand, program_storage};
use crate::machine::model::UntypedKey;
use crate::machine::model::ast::{DispatchShape, ExpressionPart, KExpression};
use crate::parse::parse;
use crate::source::Spanned;

// ---------- spec ⟺ registration consistency ----------

/// Every bucket key the seeded root registers a callable under.
fn live_buckets() -> HashSet<UntypedKey> {
    let program = crate::machine::core::program_storage();
    let storage = crate::machine::core::run_root_storage();
    let run = crate::builtins::test_support::TestRun::silent(&program, &storage);
    run.scope
        .ancestors()
        .flat_map(|scope| {
            scope
                .bindings()
                .functions()
                .iter()
                .map(|(key, _)| key.to_vec())
                .collect::<Vec<_>>()
        })
        .collect()
}

/// A spec key rendered for a failure message: keywords verbatim, slots as `_`.
fn render_key(key: &[super::KeyElementSpec]) -> Vec<String> {
    key.iter()
        .map(|element| match element {
            super::KeyElementSpec::Keyword(k) => (*k).to_string(),
            super::KeyElementSpec::Slot => "_".to_string(),
        })
        .collect()
}

/// Build a bucket-shaped `KExpression` from a spec key (keywords verbatim, slots as bare
/// identifiers) so its cached `DispatchShape` can be inspected.
fn expression_for_key<'a>(brand: RegionBrand<'a>, spec: &BinderSpec) -> KExpression<'a> {
    KExpression::new_from_iter(
        brand,
        spec.key.iter().map(|element| match element {
            super::KeyElementSpec::Keyword(k) => Spanned::bare(kw_part(k)),
            super::KeyElementSpec::Slot => Spanned::bare(ExpressionPart::Identifier("x")),
        }),
    )
}

/// Every spec entry names a bucket the seeded root actually registers, and every spec key
/// classifies `Keyworded`. Recomputed independently from the seeded root, so it is not a tautology
/// against the table: a spec key whose builtin was renamed, re-shaped, or dropped fails here.
#[test]
fn spec_table_matches_live_registration() {
    let program = program_storage();
    let brand = program.brand();
    let live = live_buckets();

    for spec in BINDER_SPECS {
        assert!(
            live.iter().any(|key| spec.matches_key(key)),
            "spec key {:?} has no registered bucket",
            render_key(spec.key)
        );

        assert_eq!(
            expression_for_key(brand.region(), spec).shape(),
            DispatchShape::Keyworded,
            "spec key {:?} does not classify Keyworded",
            render_key(spec.key)
        );
    }
}

/// Every spec entry that installs anything declares at least one channel, and a `names` entry
/// carries the bind kind the placeholder is tagged with. Pins that the table's two channels are
/// the only routes into an install.
#[test]
fn spec_channels_cover_every_installing_entry() {
    let silent: Vec<&[super::KeyElementSpec]> = BINDER_SPECS
        .iter()
        .filter(|spec| spec.installs_nothing())
        .map(|spec| spec.key)
        .collect();
    assert_eq!(
        silent.len(),
        1,
        "`VAL` is the one declaration form with no install channel; a second silent entry means a \
         binder builtin lost its extractor",
    );
    assert!(
        silent[0]
            .first()
            .is_some_and(|element| matches!(element, super::KeyElementSpec::Keyword("VAL"))),
        "the silent entry must be `VAL`",
    );
}

/// The `OperatorDef` marker agrees with the keys it labels: a spec entry is marked iff its key
/// names the `OP` declarator keyword. The marker is what `GROUP`'s member scan keys on, so a new
/// operator surface that forgets it — or a non-operator form that wrongly carries it — fails here
/// rather than silently changing which body statements a group treats as members.
#[test]
fn operator_def_marker_agrees_with_the_keys_it_labels() {
    for spec in BINDER_SPECS {
        let names_op = spec
            .key
            .iter()
            .any(|element| matches!(element, super::KeyElementSpec::Keyword("OP")));
        assert_eq!(
            spec.surface == super::BinderSurface::OperatorDef,
            names_op,
            "spec key {:?} disagrees with its surface marker",
            render_key(spec.key),
        );
    }
}

// ---------- per-statement binder plan ----------

/// The lone top-level statement `src` parses to, with its cache filled, built into `brand`'s
/// region.
fn parse_one<'a>(brand: ProgramBrand<'a>, src: &str) -> KExpression<'a> {
    parse(brand, &crate::machine::model::LabelInterner::new(), src)
        .expect("parse")
        .into_iter()
        .next()
        .expect("one statement")
}

fn name_of(key: StoredBinderKey<'_>) -> Option<String> {
    key.name.map(|(name, _)| name.to_string())
}

/// A redundant single-`Expression` paren wrapper is the same statement, so it carries the child's
/// plan through, with no aggregation.
#[test]
fn redundant_parens_pass_through() {
    let program = program_storage();
    let brand = program.brand();
    let inner = parse_one(brand, "LET x = 1");
    let wrapped = KExpression::new(
        brand.region(),
        &[Spanned::bare(ExpressionPart::Expression(
            brand.nested_node(inner.parts),
        ))],
    );
    let child = match wrapped.parts[0].value {
        ExpressionPart::Expression(child) => child,
        _ => panic!("built a single-Expression wrapper"),
    };
    assert_eq!(
        name_of(child.binder_plan().expect("the child is the binder")),
        Some("x".to_string()),
    );
    assert!(
        wrapped.binder_plan().is_none(),
        "the wrapper is not itself a binder; the submission path reads through it",
    );
}

/// A statement's plan is its own spine and nothing else: what a slot's child would install is not
/// part of it, so the namespace a block introduces is legible from its statement keys alone. These
/// shapes are rejected at submission now (a binder is not a value position) — the point here is
/// that the parse-time read never reaches into the slot in the first place.
#[test]
fn a_statements_plan_is_its_own_spine() {
    let program = program_storage();
    let brand = program.brand();
    for source in [
        "LET make_set = (FN (MAKESET item :Number) -> Number = (item))",
        "LET z = (LET a = 3)",
        "LET f = (FN (g :Number) -> Number = (LET inner = 1))",
    ] {
        let stmt = parse_one(brand, source);
        let key = stmt.binder_plan().expect("a LET is a binder");
        assert_eq!(
            name_of(key),
            Some(source.split_whitespace().nth(1).unwrap().to_string()),
            "{source}",
        );
        assert_eq!(
            key.buckets.map_or(0, |keys| keys.len()),
            0,
            "the outer LET declares no bucket of its own: {source}",
        );
    }
}

/// The spec table's `name_slot` agrees with the name extractors: for every parsed binder form
/// whose plan carries a name, the token at the cached `binder_name_slot` position IS that name;
/// `VAL` declares at its slot while installing nothing; the bucket-only forms cache no position.
#[test]
fn name_slot_agrees_with_the_extractors() {
    let program = program_storage();
    let brand = program.brand();
    for source in [
        "LET x = 1",
        "LET Alias = Number",
        "MODULE m = (LET a = 1)",
        "SIG Sx = (VAL zero :Number)",
        "UNION Ux = (Red | Green)",
        "NEWTYPE Nx = Number",
        "TYPE Tx",
        "LET double = FN (DOUBLE n :Number) -> Number = (n * 2)",
        "LET plus = OP #(⊕) OVER Number = (left + right)",
    ] {
        let stmt = parse_one(brand, source);
        let pos = stmt
            .binder_name_slot()
            .unwrap_or_else(|| panic!("a name-bearing binder form caches its position: {source}"));
        let expected = name_of(stmt.binder_plan().expect("each form installs a name"))
            .expect("each form installs a name");
        let token = match stmt.parts[pos].value {
            ExpressionPart::Identifier(s) => s.to_string(),
            ExpressionPart::Type(t) => t.as_str().to_string(),
            other => panic!("name slot holds a bare name token, got {other:?}: {source}"),
        };
        assert_eq!(token, expected, "{source}");
    }
    // `VAL` declares at its slot without installing; the bucket-only forms cache no position.
    let val = parse_one(brand, "VAL x :Number");
    assert_eq!(val.binder_name_slot(), Some(1));
    assert!(matches!(
        val.parts[1].value,
        ExpressionPart::Identifier("x")
    ));
    for source in [
        "FN (TRIPLE n :Number) -> Number = (n * 3)",
        "OP #(⊗) OVER Number = (left * right)",
        "UNARY OP #(⊖) OVER Number -> Number = (0 - operands)",
    ] {
        let stmt = parse_one(brand, source);
        assert_eq!(stmt.binder_name_slot(), None, "{source}");
    }
}

/// A `VAL` declaration installs nothing: its spec entry has no channel, so its parse-time plan is
/// `None`.
#[test]
fn val_installs_nothing() {
    let program = program_storage();
    let brand = program.brand();
    let stmt = parse_one(brand, "VAL x :Number");
    assert!(stmt.binder_plan().is_none());
}

/// Each combined statement form's own plan fills both channels: the LET value name and the bucket
/// key(s) the declaration's body registers. `LET … = UNARY OP …` is the two-bucket maximum.
#[test]
fn combined_forms_install_both_channels() {
    let program = program_storage();
    let brand = program.brand();
    for (source, buckets) in [
        (
            "LET double = FN (DOUBLE n :Number) -> Number = (n * 2)",
            1usize,
        ),
        ("LET plus = OP #(⊕) OVER Number = (left + right)", 1),
        ("LET near = OP #(≺) OVER Number -> Bool = (left < right)", 1),
        (
            "LET collect = UNARY OP #(~) OVER Number -> :(LIST OF Number) = (operands)",
            2,
        ),
    ] {
        let stmt = parse_one(brand, source);
        let key = stmt.binder_plan().expect("a combined form is a binder");
        assert_eq!(
            key.name.map(|(name, _)| name),
            Some(source.split_whitespace().nth(1).unwrap()),
            "{source}",
        );
        assert_eq!(
            key.buckets.map_or(0, |keys| keys.len()),
            buckets,
            "{source}"
        );
    }
}

/// The anonymous `FN :{…}` signature names no bucket, so a statement carrying it installs only what
/// its own name channel gives — nothing, for the bare form.
#[test]
fn anonymous_fn_installs_nothing() {
    let program = program_storage();
    let brand = program.brand();
    let stmt = parse_one(brand, "FN :{n :Number} -> Number = (n)");
    assert!(stmt.binder_plan().is_none());
}
