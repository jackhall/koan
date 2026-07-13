//! `TypeMemoCache` capacity/eviction/recency at the struct level, plus [`memo_safe`]'s
//! coverage of the pointer-transient (unsealed `RecursiveSet`) hazard.

use super::*;
use crate::machine::core::ScopeId;
use crate::machine::model::types::{KKind, NominalMember, NominalSchema, RecursiveSet};

fn cache(capacity: usize) -> TypeMemoCache {
    TypeMemoCache::new(NonZeroUsize::new(capacity).expect("test capacities are nonzero"))
}

#[test]
fn put_get_roundtrip_returns_the_stored_verdict() {
    let mut c = cache(2);
    c.insert(TypeDigest(1), TypeDigest(2), Relation::MoreSpecific, true);
    assert_eq!(
        c.lookup(TypeDigest(1), TypeDigest(2), Relation::MoreSpecific),
        Some(true)
    );
}

#[test]
fn miss_on_an_absent_key_returns_none() {
    let mut c = cache(2);
    assert_eq!(
        c.lookup(TypeDigest(1), TypeDigest(2), Relation::MoreSpecific),
        None
    );
}

#[test]
fn both_outcomes_are_stored_and_read_back() {
    let mut c = cache(4);
    c.insert(TypeDigest(1), TypeDigest(2), Relation::MoreSpecific, true);
    c.insert(TypeDigest(3), TypeDigest(4), Relation::MoreSpecific, false);
    assert_eq!(
        c.lookup(TypeDigest(1), TypeDigest(2), Relation::MoreSpecific),
        Some(true)
    );
    assert_eq!(
        c.lookup(TypeDigest(3), TypeDigest(4), Relation::MoreSpecific),
        Some(false)
    );
}

#[test]
fn same_digest_pair_under_different_relations_are_distinct_keys() {
    let mut c = cache(4);
    c.insert(TypeDigest(1), TypeDigest(2), Relation::MoreSpecific, true);
    c.insert(TypeDigest(1), TypeDigest(2), Relation::SigSatisfies, false);
    assert_eq!(
        c.lookup(TypeDigest(1), TypeDigest(2), Relation::MoreSpecific),
        Some(true)
    );
    assert_eq!(
        c.lookup(TypeDigest(1), TypeDigest(2), Relation::SigSatisfies),
        Some(false)
    );
}

#[test]
fn eviction_drops_the_least_recently_used_entry_at_capacity() {
    let mut c = cache(2);
    c.insert(TypeDigest(1), TypeDigest(1), Relation::MoreSpecific, true);
    c.insert(TypeDigest(2), TypeDigest(2), Relation::MoreSpecific, true);
    // Inserting a third entry over a capacity-2 cache evicts digest-1 (least recently used —
    // neither touched since insert, so insertion order breaks the tie).
    c.insert(TypeDigest(3), TypeDigest(3), Relation::MoreSpecific, true);
    assert_eq!(
        c.lookup(TypeDigest(1), TypeDigest(1), Relation::MoreSpecific),
        None,
        "the oldest entry is evicted"
    );
    assert_eq!(
        c.lookup(TypeDigest(2), TypeDigest(2), Relation::MoreSpecific),
        Some(true)
    );
    assert_eq!(
        c.lookup(TypeDigest(3), TypeDigest(3), Relation::MoreSpecific),
        Some(true)
    );
}

#[test]
fn a_lookup_bumps_recency_and_saves_the_entry_from_eviction() {
    let mut c = cache(2);
    c.insert(TypeDigest(1), TypeDigest(1), Relation::MoreSpecific, true);
    c.insert(TypeDigest(2), TypeDigest(2), Relation::MoreSpecific, true);
    // Touch digest-1 so it becomes the most-recently-used entry.
    assert_eq!(
        c.lookup(TypeDigest(1), TypeDigest(1), Relation::MoreSpecific),
        Some(true)
    );
    // digest-2 is now the least-recently-used entry and is evicted instead.
    c.insert(TypeDigest(3), TypeDigest(3), Relation::MoreSpecific, true);
    assert_eq!(
        c.lookup(TypeDigest(1), TypeDigest(1), Relation::MoreSpecific),
        Some(true),
        "recently-read entry survives"
    );
    assert_eq!(
        c.lookup(TypeDigest(2), TypeDigest(2), Relation::MoreSpecific),
        None,
        "least-recently-used entry is evicted"
    );
}

/// An unsealed set: created with pending members but never filled, so `digest()` stays `None`
/// — the pointer-transient window `memo_safe` must refuse. Mirrors the fixture shape in
/// `type_digest/tests.rs::multi_member_set_seals_digest_on_last_fill`.
fn unsealed_set() -> std::rc::Rc<RecursiveSet<'static>> {
    std::rc::Rc::new(RecursiveSet::new(vec![NominalMember::pending(
        "Pending".into(),
        ScopeId::SENTINEL,
        KKind::NewType,
    )]))
}

fn sealed_set() -> std::rc::Rc<RecursiveSet<'static>> {
    RecursiveSet::singleton(
        "Sealed".into(),
        ScopeId::SENTINEL,
        NominalSchema::NewType(Box::new(KType::Number)),
    )
}

#[test]
fn memo_safe_true_for_sealed_composites() {
    assert!(memo_safe(&KType::list(Box::new(KType::Number))));
    assert!(memo_safe(&KType::dict(
        Box::new(KType::Str),
        Box::new(KType::Number)
    )));
    assert!(memo_safe(&KType::union_of(vec![KType::Number, KType::Str])));
    assert!(memo_safe(&KType::function_type(
        crate::machine::model::types::Record::from_pairs(vec![("x".to_string(), KType::Number)]),
        Box::new(KType::Str)
    )));
}

#[test]
fn memo_safe_true_for_leaves() {
    assert!(memo_safe(&KType::Number));
    assert!(memo_safe(&KType::Any));
    assert!(memo_safe(&KType::Identifier));
}

#[test]
fn memo_safe_true_for_a_sealed_set_ref() {
    let set = sealed_set();
    assert!(set.digest().is_some());
    assert!(memo_safe(&KType::SetRef { set, index: 0 }));
}

#[test]
fn memo_safe_false_for_an_unsealed_set_ref() {
    let set = unsealed_set();
    assert!(set.digest().is_none());
    assert!(!memo_safe(&KType::SetRef { set, index: 0 }));
}

#[test]
fn memo_safe_false_for_an_unsealed_recursive_group() {
    let set = unsealed_set();
    assert!(!memo_safe(&KType::RecursiveGroup(set)));
}

#[test]
fn memo_safe_false_for_a_composite_containing_an_unsealed_set_ref() {
    let set = unsealed_set();
    let inner = KType::SetRef { set, index: 0 };
    let list = KType::list(Box::new(inner));
    assert!(
        !memo_safe(&list),
        "an unsealed set nested inside a composite still poisons the whole type"
    );
}
