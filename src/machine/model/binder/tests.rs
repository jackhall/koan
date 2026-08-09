//! Binder-model tests: the spec⟺registration consistency pin (the table matches the live builtin
//! function table) and the parse-time binder plan each statement caches.

use std::collections::HashMap;

use super::{BinderSpec, StoredBinderKey, BINDER_SPECS};
use crate::machine::core::{program_storage, ProgramBrand, RegionBrand};
use crate::machine::model::ast::{DispatchShape, ExpressionPart, KExpression};
use crate::machine::model::UntypedKey;
use crate::parse::parse;
use crate::source::Spanned;

// ---------- spec ⟺ registration consistency ----------

/// One live bucket as read off the seeded root: whether any overload carries a binder hook.
struct LiveBucket {
    has_hook: bool,
}

/// Whether an overload is a binder-introducing builtin — the `binder` bool `KFunction` carries.
fn overload_has_hook(f: &crate::machine::KFunction<'_>) -> bool {
    f.binder
}

/// Walk the seeded root's registered function buckets into a `key -> LiveBucket` map, recomputing
/// each bucket's hook flag straight from the live `KFunction`s.
fn live_buckets() -> HashMap<UntypedKey, LiveBucket> {
    let program = crate::machine::core::program_storage();
    let storage = crate::machine::core::run_root_storage();
    let run = crate::builtins::test_support::TestRun::silent(&program, &storage);
    let mut table: HashMap<UntypedKey, LiveBucket> = HashMap::new();
    for scope in run.scope.ancestors() {
        // Snapshot the bucket's dormant carriers, then read each under the scope's own pin — the
        // bucket stores seals, so a signature walk opens rather than dereferences.
        let buckets: Vec<(UntypedKey, Vec<_>)> = scope
            .bindings()
            .functions()
            .iter()
            .map(|(key, overloads)| {
                (
                    key.clone(),
                    overloads
                        .iter()
                        .filter_map(|slot| slot.sealed())
                        .map(|entry| scope.read_function(&entry.sealed, overload_has_hook))
                        .collect(),
                )
            })
            .collect();
        for (key, overloads) in buckets {
            let has_hook = overloads.iter().copied().any(|hook| hook);
            table.insert(key.clone(), LiveBucket { has_hook });
        }
    }
    table
}

/// Build a bucket-shaped `KExpression` from a spec key (keywords verbatim, slots as bare
/// identifiers) so its cached `DispatchShape` can be inspected.
fn expression_for_key<'a>(brand: RegionBrand<'a>, spec: &BinderSpec) -> KExpression<'a> {
    let parts = spec
        .key
        .iter()
        .map(|element| match element {
            super::UntypedElementSpec::Keyword(k) => Spanned::bare(ExpressionPart::Keyword(k)),
            super::UntypedElementSpec::Slot => Spanned::bare(ExpressionPart::Identifier("x")),
        })
        .collect();
    KExpression::new(brand, parts)
}

/// A spec entry with extractors exists for a bucket key iff that bucket carries a binder hook, and
/// every spec key classifies `Keyworded`. Recomputed independently from the seeded root, so it is
/// not a tautology against the table.
#[test]
fn spec_table_matches_live_registration() {
    let program = program_storage();
    let brand = program.brand();
    let live = live_buckets();

    // Forward: every spec entry has a matching live bucket, with the derived hook flag and shape.
    for spec in BINDER_SPECS {
        let (key, bucket) = live
            .iter()
            .find(|(key, _)| spec.matches_key(key))
            .unwrap_or_else(|| {
                panic!(
                    "spec key {:?} has no registered bucket",
                    spec.key
                        .iter()
                        .map(|e| match e {
                            super::UntypedElementSpec::Keyword(k) => (*k).to_string(),
                            super::UntypedElementSpec::Slot => "_".to_string(),
                        })
                        .collect::<Vec<_>>()
                )
            });

        if spec.installs_nothing() {
            // A declaration form (VAL): present for completeness, installs nothing, so its bucket
            // must carry no hook.
            assert!(
                !bucket.has_hook,
                "empty-extractor spec key {key:?} matches a hook-bearing bucket"
            );
        } else {
            assert!(
                bucket.has_hook,
                "spec key {key:?} has extractors but its bucket carries no binder hook"
            );
        }

        assert_eq!(
            expression_for_key(brand.region(), spec).shape(),
            DispatchShape::Keyworded,
            "spec key {key:?} does not classify Keyworded"
        );
    }

    // Reverse: every hook-bearing bucket is covered by a spec entry with extractors.
    for (key, bucket) in &live {
        if bucket.has_hook {
            let covered = BINDER_SPECS
                .iter()
                .any(|spec| !spec.installs_nothing() && spec.matches_key(key));
            assert!(
                covered,
                "hook-bearing bucket {key:?} has no BINDER_SPECS entry with extractors"
            );
        }
    }
}

/// Every spec entry that installs anything declares at least one channel, and a `names` entry
/// carries the bind kind the placeholder is tagged with. Pins that the table's two channels are
/// the only routes into an install.
#[test]
fn spec_channels_cover_every_installing_entry() {
    let silent: Vec<&[super::UntypedElementSpec]> = BINDER_SPECS
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
            .is_some_and(|element| matches!(element, super::UntypedElementSpec::Keyword("VAL"))),
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
            .any(|element| matches!(element, super::UntypedElementSpec::Keyword("OP")));
        assert_eq!(
            spec.surface == super::BinderSurface::OperatorDef,
            names_op,
            "spec key {:?} disagrees with its surface marker",
            spec.key
                .iter()
                .map(|e| match e {
                    super::UntypedElementSpec::Keyword(k) => (*k).to_string(),
                    super::UntypedElementSpec::Slot => "_".to_string(),
                })
                .collect::<Vec<_>>(),
        );
    }
}

// ---------- per-statement binder plan ----------

/// The lone top-level statement `src` parses to, with its cache filled, built into `brand`'s
/// region.
fn parse_one<'a>(brand: ProgramBrand<'a>, src: &str) -> KExpression<'a> {
    parse(brand, src)
        .expect("parse")
        .into_iter()
        .next()
        .expect("one statement")
}

fn name_of(key: StoredBinderKey<'_>) -> Option<String> {
    key.name.map(|(name, _)| name.to_string())
}

/// A redundant single-`Expression` paren wrapper is the same statement, so it carries the child's
/// plan through — the one structural exception, and no aggregation.
#[test]
fn redundant_parens_pass_through() {
    let program = program_storage();
    let brand = program.brand();
    let inner = parse_one(brand, "LET x = 1");
    let wrapped = KExpression::new(
        brand.region(),
        vec![Spanned::bare(ExpressionPart::Expression(
            brand.nested_node(inner.parts.to_vec()),
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
