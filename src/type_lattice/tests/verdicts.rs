//! The verdict table's cache behaviour.
//!
//! No law: a verdict is never load-bearing, so what the table remembers and what it forgets is
//! invisible to the algebra. These pin the table's own contract — a hit answers only for its own
//! key, a full bucket evicts the slot not touched last, the table is laid once and never grows, and
//! at its default size a realistic run of relations rarely evicts.

use crate::memory::{Bump, BumpVec};
use crate::tests::allocation_count;

use crate::type_lattice::digest::TypeDigest;
use crate::type_lattice::handle::KType;
use crate::type_lattice::order::is_subtype_of;
use crate::type_lattice::registry::{Relation, TypeRegistry};

fn key(n: u128) -> TypeDigest {
    TypeDigest(n)
}

#[test]
fn a_recorded_verdict_reads_back() {
    let bump = Bump::new();
    let types = TypeRegistry::in_region(&bump);
    assert_eq!(types.verdict(key(1), key(2), Relation::Subtype), None);
    types.record_verdict(key(1), key(2), Relation::Subtype, true);
    types.record_verdict(key(1), key(2), Relation::SigSatisfies, false);
    types.record_verdict(key(2), key(1), Relation::Subtype, false);
    assert_eq!(types.verdict(key(1), key(2), Relation::Subtype), Some(true));
    assert_eq!(
        types.verdict(key(1), key(2), Relation::SigSatisfies),
        Some(false)
    );
    assert_eq!(
        types.verdict(key(2), key(1), Relation::Subtype),
        Some(false)
    );
}

#[test]
fn a_colliding_key_never_answers_for_another() {
    let bump = Bump::new();
    // One bucket: every key collides.
    let types = TypeRegistry::in_region_with_verdict_slots(&bump, 2);
    let keys = [(1, 2, true), (2, 1, false), (3, 3, true)];
    for (subject, candidate, verdict) in keys {
        types.record_verdict(key(subject), key(candidate), Relation::Subtype, verdict);
    }
    for (subject, candidate, verdict) in keys {
        let read = types.verdict(key(subject), key(candidate), Relation::Subtype);
        assert!(read.is_none() || read == Some(verdict));
    }
    assert_eq!(types.verdict(key(1), key(2), Relation::SigSatisfies), None);
}

#[test]
fn a_full_bucket_evicts_the_slot_not_touched_last() {
    let bump = Bump::new();
    let types = TypeRegistry::in_region_with_verdict_slots(&bump, 2);
    let [a, b, c] = [key(10), key(20), key(30)];
    types.record_verdict(a, a, Relation::Subtype, true);
    types.record_verdict(b, b, Relation::Subtype, false);
    assert_eq!(types.verdict(a, a, Relation::Subtype), Some(true));
    types.record_verdict(c, c, Relation::Subtype, true);
    assert_eq!(types.verdict(a, a, Relation::Subtype), Some(true));
    assert_eq!(types.verdict(b, b, Relation::Subtype), None);
    assert_eq!(types.verdict(c, c, Relation::Subtype), Some(true));
}

#[test]
fn the_table_is_laid_once_and_never_grows() {
    let bump = Bump::new();
    drop(BumpVec::<u8>::with_capacity_in(1 << 12, &bump));
    let types = TypeRegistry::in_region_with_verdict_slots(&bump, 2);
    let before = allocation_count();
    for n in 0..100 {
        types.record_verdict(key(n), key(n + 1), Relation::Subtype, n % 2 == 0);
    }
    assert_eq!(allocation_count() - before, 0);
    assert_eq!(types.verdict_tally().0, 100);
}

/// Digests are content hashes, so which relations collide is fixed and the bound is deterministic.
#[test]
fn conflict_evictions_stay_rare_at_the_default_size() {
    let (region, scratch) = (Bump::new(), Bump::new());
    let types = TypeRegistry::in_region(&region);
    let leaves = [KType::NUMBER, KType::STR, KType::BOOL, KType::NULL];
    let mut pool = leaves.to_vec();
    for leaf in leaves {
        pool.push(types.list(leaf));
        pool.push(types.list(types.list(leaf)));
        for value in leaves {
            pool.push(types.dict(leaf, value));
        }
    }
    'pairs: for &a in &pool {
        for &b in &pool {
            if types.verdict_tally().0 >= 300 {
                break 'pairs;
            }
            is_subtype_of(&types, &scratch, a, b);
        }
    }
    let (inserts, evictions) = types.verdict_tally();
    assert!(inserts >= 300, "only {inserts} verdicts were recorded");
    assert!(
        evictions * 10 < inserts,
        "{evictions} evictions over {inserts} inserts"
    );
}
