//! The sorted-set invariant across a union.
//!
//! A sealed set is searched and merged on the promise that its ids are ascending and distinct, so
//! every union is checked against that promise directly rather than against an expected vector:
//! sortedness and distinctness are the properties the rest of the crate reads it for.

use super::*;

fn set(ids: impl IntoIterator<Item = u64>) -> SealedSet {
    let mut set = SealedSet::new();
    for id in ids {
        set.insert(SealedId(id));
    }
    set
}

/// The ids a set names, and the invariant it names them under.
fn ordered(set: &SealedSet) -> Vec<u64> {
    let ids: Vec<u64> = set.iter().map(|SealedId(id)| id).collect();
    assert!(
        ids.windows(2).all(|pair| pair[0] < pair[1]),
        "a sealed set names {ids:?}, which is not ascending and distinct"
    );
    assert_eq!(ids.len(), set.len(), "the length disagrees with the ids");
    ids
}

#[test]
fn a_union_merges_overlapping_sets() {
    let mut left = set([1, 3, 5, 7]);
    left.union_with(&set([3, 4, 5, 9]));
    assert_eq!(ordered(&left), [1, 3, 4, 5, 7, 9]);
}

#[test]
fn a_union_interleaves_disjoint_sets() {
    let mut left = set([0, 2, 4]);
    left.union_with(&set([1, 3, 5]));
    assert_eq!(ordered(&left), [0, 1, 2, 3, 4, 5]);
}

#[test]
fn a_union_with_a_nested_set_names_the_wider_one() {
    let mut wide = set([1, 2, 3, 4, 5]);
    wide.union_with(&set([2, 4]));
    assert_eq!(ordered(&wide), [1, 2, 3, 4, 5]);

    let mut narrow = set([2, 4]);
    narrow.union_with(&set([1, 2, 3, 4, 5]));
    assert_eq!(ordered(&narrow), [1, 2, 3, 4, 5]);
}

#[test]
fn a_union_with_an_empty_set_changes_nothing_either_way() {
    let mut named = set([2, 6]);
    named.union_with(&SealedSet::new());
    assert_eq!(ordered(&named), [2, 6]);

    let mut empty = SealedSet::new();
    empty.union_with(&set([2, 6]));
    assert_eq!(ordered(&empty), [2, 6]);
}
