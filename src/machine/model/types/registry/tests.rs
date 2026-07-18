//! [`TypeRegistry`] keying and verdict storage, plus [`digest_is_content`]'s coverage of the
//! pointer-transient (unsealed `RecursiveSet`) hazard.

use super::*;
use crate::machine::core::ScopeId;
use crate::machine::model::types::{KKind, NominalMember, NominalSchema, RecursiveSet};

#[test]
fn record_then_read_returns_the_stored_verdict() {
    let registry = TypeRegistry::new();
    registry.record_verdict(TypeDigest(1), TypeDigest(2), Relation::MoreSpecific, true);
    assert_eq!(
        registry.verdict(TypeDigest(1), TypeDigest(2), Relation::MoreSpecific),
        Some(true)
    );
}

#[test]
fn miss_on_an_absent_key_returns_none() {
    let registry = TypeRegistry::new();
    assert_eq!(
        registry.verdict(TypeDigest(1), TypeDigest(2), Relation::MoreSpecific),
        None
    );
}

#[test]
fn both_outcomes_are_stored_and_read_back() {
    let registry = TypeRegistry::new();
    registry.record_verdict(TypeDigest(1), TypeDigest(2), Relation::MoreSpecific, true);
    registry.record_verdict(TypeDigest(3), TypeDigest(4), Relation::MoreSpecific, false);
    assert_eq!(
        registry.verdict(TypeDigest(1), TypeDigest(2), Relation::MoreSpecific),
        Some(true)
    );
    assert_eq!(
        registry.verdict(TypeDigest(3), TypeDigest(4), Relation::MoreSpecific),
        Some(false)
    );
}

#[test]
fn same_digest_pair_under_different_relations_are_distinct_keys() {
    let registry = TypeRegistry::new();
    registry.record_verdict(TypeDigest(1), TypeDigest(2), Relation::MoreSpecific, true);
    registry.record_verdict(TypeDigest(1), TypeDigest(2), Relation::SigSatisfies, false);
    assert_eq!(
        registry.verdict(TypeDigest(1), TypeDigest(2), Relation::MoreSpecific),
        Some(true)
    );
    assert_eq!(
        registry.verdict(TypeDigest(1), TypeDigest(2), Relation::SigSatisfies),
        Some(false)
    );
}

#[test]
fn the_subject_and_candidate_positions_are_ordered() {
    let registry = TypeRegistry::new();
    registry.record_verdict(TypeDigest(1), TypeDigest(2), Relation::MoreSpecific, true);
    assert_eq!(
        registry.verdict(TypeDigest(2), TypeDigest(1), Relation::MoreSpecific),
        None,
        "the reversed pair is a different question"
    );
}

#[test]
fn a_re_record_overwrites_the_entry() {
    let registry = TypeRegistry::new();
    registry.record_verdict(TypeDigest(1), TypeDigest(2), Relation::MoreSpecific, false);
    registry.record_verdict(TypeDigest(1), TypeDigest(2), Relation::MoreSpecific, true);
    assert_eq!(
        registry.verdict(TypeDigest(1), TypeDigest(2), Relation::MoreSpecific),
        Some(true)
    );
}

#[test]
fn the_counters_track_hits_and_misses() {
    let registry = TypeRegistry::new();
    assert_eq!(registry.hit_count(), 0);
    assert_eq!(registry.miss_count(), 0);

    registry.verdict(TypeDigest(1), TypeDigest(2), Relation::MoreSpecific);
    assert_eq!(registry.miss_count(), 1);

    registry.record_verdict(TypeDigest(1), TypeDigest(2), Relation::MoreSpecific, true);
    registry.verdict(TypeDigest(1), TypeDigest(2), Relation::MoreSpecific);
    assert_eq!(registry.hit_count(), 1);
    assert_eq!(registry.miss_count(), 1);
}

#[test]
fn a_fresh_registry_shares_nothing_with_another() {
    let first = TypeRegistry::new();
    first.record_verdict(TypeDigest(1), TypeDigest(2), Relation::MoreSpecific, true);
    let second = TypeRegistry::new();
    assert_eq!(
        second.verdict(TypeDigest(1), TypeDigest(2), Relation::MoreSpecific),
        None,
        "verdicts are scoped to the run that recorded them"
    );
}

/// An unsealed set: created with pending members but never filled, so `digest()` stays `None`
/// — the pointer-transient window `digest_is_content` must refuse. Mirrors the fixture shape in
/// `type_digest/tests.rs::multi_member_set_seals_digest_on_last_fill`.
fn unsealed_set() -> std::rc::Rc<RecursiveSet> {
    std::rc::Rc::new(RecursiveSet::new(vec![NominalMember::pending(
        "Pending".into(),
        ScopeId::SENTINEL,
        KKind::NewType,
    )]))
}

fn sealed_set() -> std::rc::Rc<RecursiveSet> {
    RecursiveSet::singleton(
        "Sealed".into(),
        ScopeId::SENTINEL,
        NominalSchema::NewType(Box::new(KType::Number)),
    )
}

#[test]
fn digest_is_content_true_for_sealed_composites() {
    assert!(digest_is_content(&KType::list(Box::new(KType::Number))));
    assert!(digest_is_content(&KType::dict(
        Box::new(KType::Str),
        Box::new(KType::Number)
    )));
    assert!(digest_is_content(&KType::union_of(vec![
        KType::Number,
        KType::Str
    ])));
    assert!(digest_is_content(&KType::function_type(
        crate::machine::model::types::Record::from_pairs(vec![("x".to_string(), KType::Number)]),
        Box::new(KType::Str)
    )));
}

#[test]
fn digest_is_content_true_for_leaves() {
    assert!(digest_is_content(&KType::Number));
    assert!(digest_is_content(&KType::Any));
    assert!(digest_is_content(&KType::Identifier));
}

#[test]
fn digest_is_content_true_for_a_sealed_set_ref() {
    let set = sealed_set();
    assert!(set.digest().is_some());
    assert!(digest_is_content(&KType::SetRef { set, index: 0 }));
}

#[test]
fn digest_is_content_false_for_an_unsealed_set_ref() {
    let set = unsealed_set();
    assert!(set.digest().is_none());
    assert!(!digest_is_content(&KType::SetRef { set, index: 0 }));
}

#[test]
fn digest_is_content_false_for_an_unsealed_recursive_group() {
    let set = unsealed_set();
    assert!(!digest_is_content(&KType::RecursiveGroup(set)));
}

#[test]
fn digest_is_content_false_for_a_composite_containing_an_unsealed_set_ref() {
    let set = unsealed_set();
    let inner = KType::SetRef { set, index: 0 };
    let list = KType::list(Box::new(inner));
    assert!(
        !digest_is_content(&list),
        "an unsealed set nested inside a composite still poisons the whole type"
    );
}
