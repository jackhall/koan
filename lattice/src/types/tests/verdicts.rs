//! The verdict tables' cache behaviour.
//!
//! No law: a verdict is never load-bearing, so what a table remembers is invisible to the algebra.
//! These pin the tables' own contract: a hit answers only for its own key, a verdict over two load
//! types is read from the load's table alone, and the load's table keeps every verdict the load
//! recorded.

use crate::bump::Bump;
use crate::symbols::{BinderSymbol, SymbolInterner};

use crate::types::chain::TypeChain;
use crate::types::digest::TypeDigest;
use crate::types::handle::{Handle, KType, TypeHandle};
use crate::types::tiers::{BumpHalf, RootBuilder};
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
    let (branch, root) = (types.branch_verdict_probes(), types.load_probes());
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
        types.load_probes(),
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
    let root = types.load_probes();
    types.verdict(minted.raw(), KType::NUMBER.raw(), Relation::Subtype);
    assert_eq!(
        types.load_probes(),
        root,
        "a branch pair never reads the root's table"
    );
}

#[test]
fn a_shared_fold_never_answers_for_another_key() {
    let bump = Bump::new();
    let types = crate::types::tests::building(&bump);
    // `(n, n)` folds to `n.rotate_left(32) ^ n`: these three pairs share their low bits.
    let keys = [(1, 2, true), (2, 1, false), (3, 3, true)];
    for (subject, candidate, verdict) in keys {
        types.record_verdict(key(subject), key(candidate), Relation::Subtype, verdict);
    }
    for (subject, candidate, verdict) in keys {
        let read = types.verdict(key(subject), key(candidate), Relation::Subtype);
        assert_eq!(read, Some(verdict));
    }
    assert_eq!(types.verdict(key(1), key(2), Relation::Fits), None);
}

#[test]
fn the_loads_verdict_table_keeps_every_verdict() {
    let (load, region) = (Bump::new(), Bump::new());
    let builder = RootBuilder::in_region(&load);
    let building = TypeChain::building(builder);
    let seeded = building.load_verdict_count();
    let n = 4096;
    for i in 0..n {
        building.record_verdict(key(i), key(i + 1), Relation::Subtype, i % 3 == 0);
    }
    assert_eq!(building.load_verdict_count() - seeded, n as usize);
    let types = TypeChain::new(builder.freeze(), BumpHalf::new(&region));
    for i in 0..n {
        assert_eq!(
            types.verdict(key(i), key(i + 1), Relation::Subtype),
            Some(i % 3 == 0)
        );
    }
}
