//! Binder-model tests: the spec⟺registration consistency pin (the table matches the live builtin
//! function table) and the parse-time aggregation of `binder_installs` over the position rule.

use std::collections::HashMap;

use super::{BinderExtract, BinderKey, BinderSpec, BINDER_SPECS};
use crate::machine::model::ast::{DispatchShape, ExpressionPart, KExpression};
use crate::machine::model::{KType, SignatureElement, UntypedKey};
use crate::parse::parse;
use crate::source::Spanned;

// ---------- spec ⟺ registration consistency ----------

/// One live bucket as read off the seeded root: whether any overload carries a binder hook, and —
/// if so — the chain-slot mask recomputed independently from the hook-bearing overloads' signatures
/// (the `!= KEXPRESSION` AND rule, keyword positions false).
struct LiveBucket {
    has_hook: bool,
    recomputed_mask: Option<Vec<bool>>,
}

/// Phase 3: `KFunction` replaces the `binder_name` / `binder_bucket` hook fields with a `binder`
/// bool. When it does, this predicate reads that bool instead.
fn overload_has_hook(f: &crate::machine::KFunction<'_>) -> bool {
    f.binder_name.is_some() || f.binder_bucket.is_some()
}

/// The per-overload eager-slot mask: an `Argument` slot is `true` iff its ktype is not
/// `KEXPRESSION`; keyword positions are `false`.
fn overload_mask(f: &crate::machine::KFunction<'_>) -> Vec<bool> {
    f.signature
        .elements
        .iter()
        .map(|element| match element {
            SignatureElement::Argument(arg) => arg.ktype != KType::KEXPRESSION,
            SignatureElement::Keyword(_) => false,
        })
        .collect()
}

/// Walk the seeded root's registered function buckets into a `key -> LiveBucket` map, recomputing
/// each bucket's hook flag and (AND-folded) binder mask straight from the live `KFunction`s.
fn live_buckets() -> HashMap<UntypedKey, LiveBucket> {
    let storage = crate::machine::core::run_root_storage();
    let run = crate::builtins::test_support::TestRun::silent(&storage);
    let mut table: HashMap<UntypedKey, LiveBucket> = HashMap::new();
    for scope in run.scope.ancestors() {
        for (key, overloads) in scope.bindings().functions().iter() {
            let has_hook = overloads.iter().any(|(f, _)| overload_has_hook(f));
            let recomputed_mask = overloads
                .iter()
                .filter(|(f, _)| overload_has_hook(f))
                .map(|(f, _)| overload_mask(f))
                .reduce(|acc, next| {
                    acc.into_iter()
                        .zip(next)
                        .map(|(a, b)| a && b)
                        .collect::<Vec<bool>>()
                });
            table.insert(
                key.clone(),
                LiveBucket {
                    has_hook,
                    recomputed_mask,
                },
            );
        }
    }
    table
}

/// Build a bucket-shaped `KExpression` from a spec key (keywords verbatim, slots as bare
/// identifiers) so its cached `DispatchShape` can be inspected.
fn expression_for_key(spec: &BinderSpec) -> KExpression<'static> {
    let parts = spec
        .key
        .iter()
        .map(|element| match element {
            super::UntypedElementSpec::Keyword(k) => {
                Spanned::bare(ExpressionPart::Keyword((*k).to_string()))
            }
            super::UntypedElementSpec::Slot => {
                Spanned::bare(ExpressionPart::Identifier("x".to_string()))
            }
        })
        .collect();
    KExpression::new(parts)
}

/// A spec entry with extractors exists for a bucket key iff that bucket carries a binder hook; each
/// spec mask equals the mask recomputed from the live signatures; and every spec key classifies
/// `Keyworded`. Recomputed independently from the seeded root, so it is not a tautology against the
/// table.
#[test]
fn spec_table_matches_live_registration() {
    let live = live_buckets();

    // Forward: every spec entry has a matching live bucket, with the derived mask and shape.
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

        if spec.extractors.is_empty() {
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
            assert_eq!(
                bucket.recomputed_mask.as_deref(),
                Some(spec.chain_slot_mask),
                "spec chain_slot_mask disagrees with the mask recomputed from live signatures for {key:?}"
            );
        }

        assert_eq!(
            expression_for_key(spec).shape(),
            DispatchShape::Keyworded,
            "spec key {key:?} does not classify Keyworded"
        );
    }

    // Reverse: every hook-bearing bucket is covered by a spec entry with extractors.
    for (key, bucket) in &live {
        if bucket.has_hook {
            let covered = BINDER_SPECS
                .iter()
                .any(|spec| !spec.extractors.is_empty() && spec.matches_key(key));
            assert!(
                covered,
                "hook-bearing bucket {key:?} has no BINDER_SPECS entry with extractors"
            );
        }
    }
}

/// The `BinderExtract::run` channel splits by shape: `Name` produces a `BinderKey::Name`, `Bucket`
/// a `BinderKey::Bucket`. Pins that the table's two extractor kinds route to the two install
/// channels.
#[test]
fn extractor_channels_route_to_install_kinds() {
    for spec in BINDER_SPECS {
        for extract in spec.extractors {
            match extract {
                BinderExtract::Name(..) => {}
                BinderExtract::Bucket(..) => {}
            }
        }
    }
}

// ---------- parse-time aggregation ----------

/// The lone top-level statement `src` parses to, with its cache filled.
fn parse_one(src: &str) -> KExpression<'static> {
    parse(src)
        .expect("parse")
        .into_iter()
        .next()
        .expect("one statement")
}

fn names(installs: &[BinderKey]) -> Vec<String> {
    installs
        .iter()
        .filter_map(|k| match k {
            BinderKey::Name(name, _) => Some(name.clone()),
            BinderKey::Bucket(_) => None,
        })
        .collect()
}

fn bucket_count(installs: &[BinderKey]) -> usize {
    installs
        .iter()
        .filter(|k| matches!(k, BinderKey::Bucket(_)))
        .count()
}

/// A LET whose value slot holds an FN aggregates both the LET name and the FN bucket, because the
/// LET value slot is a chain slot and the FN child is not block-shaped.
#[test]
fn nested_chain_aggregates_both_keys() {
    let stmt = parse_one("LET make_set = (FN (MAKESET item :Number) -> Number = (item))");
    assert_eq!(names(stmt.binder_installs()), vec!["make_set".to_string()]);
    assert_eq!(bucket_count(stmt.binder_installs()), 1);
}

/// A binder chain of two LETs aggregates both names: the outer LET's value slot is a chain slot and
/// the inner LET is not block-shaped.
#[test]
fn nested_let_chain_aggregates_both_names() {
    let stmt = parse_one("LET z = (LET a = 3)");
    let mut got = names(stmt.binder_installs());
    got.sort();
    assert_eq!(got, vec!["a".to_string(), "z".to_string()]);
}

/// A redundant single-`Expression` paren wrapper passes its child's aggregate straight through.
#[test]
fn redundant_parens_pass_through() {
    let inner = parse_one("LET x = 1");
    let expected = names(inner.binder_installs());
    let wrapped = KExpression::new(vec![Spanned::bare(ExpressionPart::Expression(Box::new(
        inner,
    )))]);
    assert_eq!(names(wrapped.binder_installs()), expected);
    assert_eq!(names(wrapped.binder_installs()), vec!["x".to_string()]);
}

/// A block-shaped child on a chain slot is cut off: the outer LET installs only its own name, not
/// the binders inside the block.
#[test]
fn block_child_cut_off() {
    let stmt = parse_one("LET x = (LET a = 1  LET b = 2)");
    assert_eq!(names(stmt.binder_installs()), vec!["x".to_string()]);
}

/// A lazy (`:KExpression`) body slot is cut off by its `false` mask entry: the FN body's inner LET
/// does not join the aggregate.
#[test]
fn lazy_body_cut_off() {
    let stmt = parse_one("LET f = (FN (g :Number) -> Number = (LET inner = 1))");
    assert_eq!(names(stmt.binder_installs()), vec!["f".to_string()]);
    // The FN bucket still aggregates; only the lazy body is cut off.
    assert_eq!(bucket_count(stmt.binder_installs()), 1);
}

/// A quote on a chain slot is not an `Expression` part, so it is cut off.
#[test]
fn quote_cut_off() {
    let stmt = parse_one("LET q = #(LET x = 1)");
    assert_eq!(names(stmt.binder_installs()), vec!["q".to_string()]);
    assert_eq!(bucket_count(stmt.binder_installs()), 0);
}

/// A list literal on a chain slot is not an `Expression` part, so its elements are cut off.
#[test]
fn literal_elements_cut_off() {
    let stmt = parse_one("LET lst = [1 (LET y = 2)]");
    assert_eq!(names(stmt.binder_installs()), vec!["lst".to_string()]);
}

/// A `VAL` declaration installs nothing: its spec entry has empty extractors, so its parse-time
/// plan is `None` and it aggregates no install.
#[test]
fn val_installs_nothing() {
    let stmt = parse_one("VAL x :Number");
    assert!(stmt.binder_plan().is_none());
    assert!(stmt.binder_installs().is_empty());
}
