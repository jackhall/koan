//! Lazy-slot model tests: the spec⟺registration consistency pin (the table matches the live
//! builtin signatures) and the seal-time stamp each parsed form carries.

use std::collections::BTreeMap;

use super::{LAZY_SLOT_SPECS, LazyKinds, LazySlotSpec};
use crate::builtins::test_support::TestRun;
use crate::machine::core::{program_storage, run_root_storage};
use crate::machine::model::key_spec::{key_matches_untyped, key_specs_agree, render_key};
use crate::machine::model::{KType, SignatureElement, TypeNode, TypeRegistry, UntypedKey};
use crate::parse::parse;

/// The kind an exact raw-capture slot type stands for; `None` for a slot type that captures
/// nothing raw. A name-token carrier answers `None`: a bare token is not an eager shape, so it
/// never stages and needs no stamp to stay raw.
fn exact_kind_of(ktype: KType) -> Option<LazyKinds> {
    match ktype {
        KType::KEXPRESSION => Some(LazyKinds::CODE),
        KType::SIGILED_TYPE_EXPR => Some(LazyKinds::TYPE_EXPR),
        KType::RECORD_TYPE => Some(LazyKinds::RECORD_TYPE),
        _ => None,
    }
}

/// The kinds a slot type stands for, distributed over union members: a union carrier slot admits
/// every carrier spelling it lists, so its bucket's stamp must carry each member's kind. This is
/// what forces a union-slot builtin's bucket to declare a correct [`LAZY_SLOT_SPECS`] entry.
fn kind_of(ktype: KType, types: &TypeRegistry) -> Option<LazyKinds> {
    if let Some(kind) = exact_kind_of(ktype) {
        return Some(kind);
    }
    let kinds = types.with_node(ktype, |node| match node {
        TypeNode::Union { members } => members
            .iter()
            .filter_map(|member| exact_kind_of(*member))
            .fold(LazyKinds::EMPTY, LazyKinds::with),
        _ => LazyKinds::EMPTY,
    });
    (!kinds.is_empty()).then_some(kinds)
}

/// The lazy slots every seeded builtin bucket actually declares: per bucket key, the union over the
/// bucket's overloads of each raw-capture slot's kind, keyed by slot index. Buckets declaring none
/// are absent. Derived from the live registration, so it is not a tautology against the table.
fn live_lazy_slots() -> Vec<(UntypedKey, BTreeMap<usize, LazyKinds>)> {
    let program = program_storage();
    let storage = run_root_storage();
    let run = TestRun::silent(&program, &storage);
    let types = run.registry_handle();
    let mut live = Vec::new();
    for scope in run.scope.ancestors() {
        for (key, bucket) in scope.bindings().functions().iter() {
            let mut slots: BTreeMap<usize, LazyKinds> = BTreeMap::new();
            for entry in bucket.iter() {
                let opened = entry.sealed.open_at();
                for (i, element) in opened.value().signature.elements().iter().enumerate() {
                    if let SignatureElement::Argument(argument) = element
                        && let Some(kind) = kind_of(argument.ktype, &types)
                    {
                        let slot = slots.entry(i).or_default();
                        *slot = slot.with(kind);
                    }
                }
            }
            if !slots.is_empty() {
                live.push((key.to_vec(), slots));
            }
        }
    }
    live
}

/// True iff the dispatch-miss diagnosis table reserves this spec key — the shape registers nothing,
/// so no live bucket can vouch for its lazy-slot entry.
fn reserves(key: &[crate::machine::model::key_spec::KeyElementSpec]) -> bool {
    crate::machine::model::miss_diagnostics::MISS_DIAGNOSTICS
        .iter()
        .any(|entry| entry.reserved && key_specs_agree(entry.key, key))
}

/// The entry `key` matches, or `None`.
fn spec_for(key: &UntypedKey) -> Option<&'static LazySlotSpec> {
    LAZY_SLOT_SPECS
        .iter()
        .find(|spec| key_matches_untyped(spec.key, key))
}

/// Every live builtin bucket with a raw-capture slot has a table entry declaring exactly those
/// slots and kinds. A builtin that grows, loses, or re-indexes a lazy slot fails here.
#[test]
fn every_lazy_builtin_bucket_has_a_matching_spec_entry() {
    for (key, expected) in live_lazy_slots() {
        let spec = spec_for(&key).unwrap_or_else(|| {
            panic!("live bucket with lazy slots {expected:?} has no LAZY_SLOT_SPECS entry")
        });
        let declared: BTreeMap<usize, LazyKinds> = spec
            .slots
            .iter()
            .map(|(index, kinds)| (*index, *kinds))
            .collect();
        assert_eq!(
            declared,
            expected,
            "spec key {:?} declares the wrong lazy slots",
            render_key(spec.key)
        );
    }
}

/// The other direction: no orphan entries. A table key naming no live lazy bucket — a builtin
/// renamed, re-shaped, or dropped — fails here.
///
/// A key the dispatch-miss diagnosis table **reserves** justifies its entry without a registration:
/// nothing registers there by design, and the entry is what keeps the body slot raw so the statement
/// reaches the miss the diagnosis reads instead of dying on an eagerly evaluated body first.
#[test]
fn every_spec_entry_names_a_live_lazy_bucket() {
    let live = live_lazy_slots();
    for spec in LAZY_SLOT_SPECS {
        assert!(
            live.iter()
                .any(|(key, _)| key_matches_untyped(spec.key, key))
                || reserves(spec.key),
            "spec key {:?} names no live bucket with lazy slots",
            render_key(spec.key)
        );
        assert!(
            !spec.slots.is_empty(),
            "spec key {:?} declares no lazy slot, so it does not belong in the table",
            render_key(spec.key)
        );
        assert!(
            spec.slots.windows(2).all(|w| w[0].0 < w[1].0),
            "spec key {:?} lists its slots out of ascending order",
            render_key(spec.key)
        );
    }
}

/// Every declared slot index names a slot position of its own key, and every kind set is non-empty.
#[test]
fn spec_slot_indices_name_slot_positions() {
    for spec in LAZY_SLOT_SPECS {
        for (index, kinds) in spec.slots {
            assert!(
                *index < spec.key.len(),
                "spec key {:?} declares slot {index} past its run",
                render_key(spec.key)
            );
            assert!(
                !kinds.is_empty(),
                "spec key {:?} declares an empty kind set at slot {index}",
                render_key(spec.key)
            );
            assert!(
                matches!(spec.key[*index], super::KeyElementSpec::Slot),
                "spec key {:?} declares its keyword position {index} lazy",
                render_key(spec.key)
            );
        }
    }
}

/// The derivation distributes over union carrier slots: a union-typed slot contributes every
/// member's kind, so a bucket that spells its raw capture as a union is held to a table entry
/// covering all of them. Name-token members contribute nothing — a bare token is not an eager
/// shape and never stages.
#[test]
fn the_kind_derivation_distributes_over_union_members() {
    let registries = crate::machine::model::RunRegistries::new();
    let types = &registries.types;

    let both = types.union_of(&[
        KType::TYPE_NAME_TOKEN,
        KType::SIGILED_TYPE_EXPR,
        KType::RECORD_TYPE,
        KType::IDENTIFIER,
    ]);
    assert_eq!(
        kind_of(both, types),
        Some(LazyKinds::TYPE_EXPR.with(LazyKinds::RECORD_TYPE)),
    );
    assert_eq!(
        kind_of(
            types.union_of(&[KType::TYPE_NAME_TOKEN, KType::SIGILED_TYPE_EXPR]),
            types
        ),
        Some(LazyKinds::TYPE_EXPR),
    );
    // A union of name tokens alone stages nothing, so its bucket needs no table entry at all.
    assert_eq!(
        kind_of(
            types.union_of(&[KType::TYPE_NAME_TOKEN, KType::IDENTIFIER]),
            types
        ),
        None,
    );
    assert_eq!(
        kind_of(types.union_of(&[KType::NUMBER, KType::STR]), types),
        None
    );
    // The bare constants keep their own kinds.
    assert_eq!(kind_of(KType::KEXPRESSION, types), Some(LazyKinds::CODE));
    assert_eq!(kind_of(KType::NUMBER, types), None);
}

// ---------- the seal-time stamp ----------

/// The stamp a parsed statement carries at each slot, as `(index, kinds)` for the non-empty ones.
fn stamped_slots(source: &str) -> Vec<(usize, LazyKinds)> {
    let program = program_storage();
    let brand = program.brand();
    let statement = parse(brand, &crate::machine::model::LabelInterner::new(), source)
        .expect("the snippet parses")
        .into_iter()
        .next()
        .expect("one statement");
    (0..statement.parts.len())
        .map(|i| (i, statement.lazy_kinds_at(i)))
        .filter(|(_, kinds)| !kinds.is_empty())
        .collect()
}

/// A builtin lazy form is stamped at parse, by the same construction door that fills the binder
/// caches — so the scheduler reads which children stay raw without resolving dispatch first.
#[test]
fn parsed_builtin_forms_carry_their_lazy_stamp() {
    assert_eq!(
        stamped_slots("MATCH x -> Number WITH (:| (x > 1) = 2)"),
        vec![(5, LazyKinds::CODE)],
    );
    assert_eq!(
        stamped_slots("TRY (risky) -> Number WITH (:| Error = 0)"),
        vec![(1, LazyKinds::CODE), (5, LazyKinds::CODE)],
    );
    assert_eq!(
        stamped_slots("FN (DOUBLE n :Number) -> Number = (n * 2)"),
        vec![
            (1, LazyKinds::CODE),
            (3, LazyKinds::TYPE_EXPR.with(LazyKinds::RECORD_TYPE)),
            (5, LazyKinds::CODE),
        ],
    );
    assert_eq!(
        stamped_slots("NEWTYPE Meters = :(Number)"),
        vec![(3, LazyKinds::TYPE_EXPR.with(LazyKinds::RECORD_TYPE))],
    );
    assert_eq!(
        stamped_slots("USING Shapes SCOPE (x)"),
        vec![(3, LazyKinds::CODE)]
    );
}

/// A user-defined call carries no stamp at all: lazy-slot declaration is available only to builtin
/// registration, so every argument of a user form evaluates.
#[test]
fn a_user_call_carries_no_lazy_stamp() {
    assert!(stamped_slots("MYFORM (1 + 2)").is_empty());
    assert!(stamped_slots("PRINT (1 + 2)").is_empty());
    assert!(stamped_slots("LET x = (1 + 2)").is_empty());
}
