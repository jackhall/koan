//! [`TypeRegistry`]'s two maps: node interning and reads, and verdict keying and storage.

use super::*;

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

// --- Node interning ---

/// Interning is insert-if-absent: the same content yields one node and two equal handles.
#[test]
fn interning_the_same_content_twice_yields_one_handle() {
    let registry = TypeRegistry::new();
    let first = registry.list(registry.intern(TypeNode::Number));
    let second = registry.list(registry.intern(TypeNode::Number));
    assert_eq!(first, second);
    assert_eq!(registry.nodes_snapshot().len(), {
        let baseline = TypeRegistry::new();
        baseline.list(baseline.intern(TypeNode::Number));
        baseline.nodes_snapshot().len()
    });
}

/// Distinct content lands on distinct handles.
#[test]
fn distinct_content_yields_distinct_handles() {
    let registry = TypeRegistry::new();
    let number = registry.intern(TypeNode::Number);
    let string = registry.intern(TypeNode::Str);
    assert_ne!(number, string);
    assert_ne!(registry.list(number), registry.list(string));
}

/// A handle reads back the content it was interned from.
#[test]
fn a_handle_reads_back_its_node() {
    let registry = TypeRegistry::new();
    let number = registry.intern(TypeNode::Number);
    let list = registry.list(number);
    match registry.node(list) {
        TypeNode::List { element } => assert_eq!(element, number),
        _ => panic!("a list handle names a list node"),
    }
}

/// The fixed handles are dereferenceable in a registry that has interned nothing else.
#[test]
fn the_constant_nodes_are_pre_seeded() {
    let registry = TypeRegistry::new();
    let snapshot = registry.nodes_snapshot();
    let seeded = TypeRegistry::new();
    for node in [
        TypeNode::Number,
        TypeNode::Str,
        TypeNode::Bool,
        TypeNode::Null,
        TypeNode::Identifier,
        TypeNode::KExpression,
        TypeNode::SigiledTypeExpr,
        TypeNode::RecordType,
        TypeNode::Any,
        TypeNode::OfKind(KKind::ProperType),
        TypeNode::OfKind(KKind::Signature),
        TypeNode::OfKind(KKind::AnyType),
        TypeNode::OfKind(KKind::NewType),
        TypeNode::OfKind(KKind::TypeConstructor),
    ] {
        let handle = seeded.intern(node);
        assert!(
            snapshot.contains_key(&handle.digest()),
            "a fresh registry pre-seeds every constant node"
        );
    }
    let any = seeded.intern(TypeNode::Any);
    for handle in [
        seeded.list(any),
        seeded.dict(any, any),
        seeded.signature(SigSchema::empty(), Vec::new()),
    ] {
        assert!(snapshot.contains_key(&handle.digest()));
    }
}

/// A handle that names nothing is a bug, not a state.
#[test]
#[should_panic(expected = "names no interned node")]
fn reading_an_uninterned_handle_panics() {
    let registry = TypeRegistry::new();
    registry.node(KType::from_digest(TypeDigest(0xdead_beef)));
}

/// A snapshot is taken against the table as it stood, so a walk over it is unaffected by
/// interning that happens during the walk.
#[test]
fn a_snapshot_does_not_observe_later_interning() {
    let registry = TypeRegistry::new();
    let before = registry.nodes_snapshot();
    let fresh = registry.record(Record::from_pairs(vec![(
        "x".to_string(),
        registry.intern(TypeNode::Number),
    )]));
    assert!(!before.contains_key(&fresh.digest()));
    assert!(registry.nodes_snapshot().contains_key(&fresh.digest()));
}

// --- Union canonicalization ---

/// A nested union flattens into its parent and duplicate members collapse.
#[test]
fn union_of_flattens_and_deduplicates() {
    let registry = TypeRegistry::new();
    let number = registry.intern(TypeNode::Number);
    let string = registry.intern(TypeNode::Str);
    let inner = registry.union_of(vec![number, string]);
    let outer = registry.union_of(vec![inner, number]);
    assert_eq!(
        outer, inner,
        "flattening then deduplicating recovers `inner`"
    );
    match registry.node(outer) {
        TypeNode::Union { members } => assert_eq!(members.len(), 2),
        _ => panic!("a two-member union stays a union"),
    }
}

/// A single surviving member is that member, not a one-member union.
#[test]
fn union_of_collapses_to_a_lone_member() {
    let registry = TypeRegistry::new();
    let number = registry.intern(TypeNode::Number);
    assert_eq!(registry.union_of(vec![number, number]), number);
}

// --- Join ---

#[test]
fn join_of_equal_types_is_that_type() {
    let registry = TypeRegistry::new();
    let number = registry.intern(TypeNode::Number);
    assert_eq!(registry.join(number, number), number);
}

#[test]
fn join_of_lists_joins_element_wise() {
    let registry = TypeRegistry::new();
    let number = registry.intern(TypeNode::Number);
    let string = registry.intern(TypeNode::Str);
    let any = registry.intern(TypeNode::Any);
    assert_eq!(
        registry.join(registry.list(number), registry.list(string)),
        registry.list(any)
    );
}

#[test]
fn join_of_unrelated_types_is_any() {
    let registry = TypeRegistry::new();
    let number = registry.intern(TypeNode::Number);
    let string = registry.intern(TypeNode::Str);
    assert_eq!(
        registry.join(number, string),
        registry.intern(TypeNode::Any)
    );
}

#[test]
fn join_iter_over_nothing_is_any() {
    let registry = TypeRegistry::new();
    assert_eq!(
        registry.join_iter(Vec::new()),
        registry.intern(TypeNode::Any)
    );
}
