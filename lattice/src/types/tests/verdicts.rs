//! The verdict table's cache behaviour.
//!
//! No law: a verdict is never load-bearing, so what the table remembers and what it forgets is
//! invisible to the algebra. These pin the table's own contract — a hit answers only for its own
//! key, a full bucket evicts the slot not touched last, the table is laid once and never grows, and
//! at its default size a realistic run of relations rarely evicts.

use crate::bump::{Bump, BumpVec};
use crate::symbols::{BinderSymbol, SymbolInterner};
use crate::tests::allocation_count;

use super::raw::order::is_subtype_of;
use crate::types::chain::TypeChain;
use crate::types::digest::TypeDigest;
use crate::types::handle::{Handle, KType, TypeHandle};
use crate::types::verdicts::Relation;

/// A root handle over `n`: the suite runs on the load's chain, where every handle is in-root.
fn key(n: u128) -> Handle {
    Handle::new(TypeDigest(n), true, true)
}

#[test]
fn a_recorded_verdict_reads_back() {
    let bump = Bump::new();
    let types = crate::types::tests::building(&bump);
    assert_eq!(types.verdict(key(1), key(2), Relation::Subtype), None);
    types.record_verdict(key(1), key(2), Relation::Subtype, true);
    types.record_verdict(key(1), key(2), Relation::Fits, false);
    types.record_verdict(key(2), key(1), Relation::Subtype, false);
    assert_eq!(types.verdict(key(1), key(2), Relation::Subtype), Some(true));
    assert_eq!(types.verdict(key(1), key(2), Relation::Fits), Some(false));
    assert_eq!(
        types.verdict(key(2), key(1), Relation::Subtype),
        Some(false)
    );
}

/// A record over a field named `x`, minted in a branch: a type the root does not hold.
fn branch_type<'t, H: crate::types::tiers::Habitat<'t>>(
    types: &TypeChain<'t, H>,
    symbols: &SymbolInterner,
) -> KType {
    let field = BinderSymbol::declared("x", symbols).expect("a bindable token");
    types.record(&Bump::new(), &[(field, KType::NUMBER)])
}

#[test]
fn a_verdict_over_two_root_types_reads_the_root_table_alone() {
    let symbols = SymbolInterner::new();
    let region = Bump::new();
    let types = crate::types::tests::tiered(&region);
    // The own branch's verdict table is laid first, so a read that probed it would count.
    let minted = branch_type(&types, &symbols);
    types.record_verdict(minted.raw(), KType::NUMBER.raw(), Relation::Subtype, false);
    let (branch, root) = (types.branch_verdict_probes(), types.root_probes());
    types.verdict(
        KType::NUMBER.raw(),
        KType::ANY_VALUE.raw(),
        Relation::Subtype,
    );
    assert_eq!(
        types.branch_verdict_probes(),
        branch,
        "no branch table is read"
    );
    assert_eq!(
        types.root_probes(),
        root + 1,
        "the root's table is read once"
    );
}

#[test]
fn a_verdict_over_a_branch_type_skips_the_root_table() {
    let symbols = SymbolInterner::new();
    let region = Bump::new();
    let types = crate::types::tests::tiered(&region);
    let minted = branch_type(&types, &symbols);
    let root = types.root_probes();
    types.verdict(minted.raw(), KType::NUMBER.raw(), Relation::Subtype);
    assert_eq!(
        types.root_probes(),
        root,
        "a branch pair never reads the root's table"
    );
}

#[test]
fn a_colliding_key_never_answers_for_another() {
    let bump = Bump::new();
    // One bucket: every key collides.
    let types = TypeChain::building(crate::types::tiers::RootBuilder::with_verdict_slots(
        &bump, 2,
    ));
    let keys = [(1, 2, true), (2, 1, false), (3, 3, true)];
    for (subject, candidate, verdict) in keys {
        types.record_verdict(key(subject), key(candidate), Relation::Subtype, verdict);
    }
    for (subject, candidate, verdict) in keys {
        let read = types.verdict(key(subject), key(candidate), Relation::Subtype);
        assert!(read.is_none() || read == Some(verdict));
    }
    assert_eq!(types.verdict(key(1), key(2), Relation::Fits), None);
}

#[test]
fn a_full_bucket_evicts_the_slot_not_touched_last() {
    let bump = Bump::new();
    let types = TypeChain::building(crate::types::tiers::RootBuilder::with_verdict_slots(
        &bump, 2,
    ));
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
    let types = TypeChain::building(crate::types::tiers::RootBuilder::with_verdict_slots(
        &bump, 2,
    ));
    // Seeding the code composites records verdicts of its own; only the loop's are counted.
    let seeded = types.verdict_tally().0;
    let before = allocation_count();
    for n in 0..100 {
        types.record_verdict(key(n), key(n + 1), Relation::Subtype, n % 2 == 0);
    }
    assert_eq!(allocation_count() - before, 0);
    assert_eq!(types.verdict_tally().0 - seeded, 100);
}

/// Digests are content hashes, so which relations collide is fixed and the bound is deterministic.
#[test]
fn conflict_evictions_stay_rare_at_the_default_size() {
    let (region, scratch) = (Bump::new(), Bump::new());
    let types = crate::types::tests::building(&region);
    let leaves = [
        KType::NUMBER.raw(),
        KType::STR.raw(),
        KType::BOOL.raw(),
        KType::NULL.raw(),
    ];
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
