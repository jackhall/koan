//! The memo's pointer stability: a `BumpRun` is read back through the region that wrote it, and
//! that region moves.
//!
//! `BumpRun` is the crate's second use of the argument `Resident` and `redeem` already stand on — a
//! bump allocates in pointer-stable chunks, a `Bump` moves without moving a chunk byte, and a
//! region never resets and frees its chunks only at its own drop. These run under Miri, where a
//! stale pointer or a leaked chunk is a failure rather than a coincidence.

use super::*;

fn ids(indices: impl IntoIterator<Item = u32>) -> Vec<SealedId> {
    indices
        .into_iter()
        .map(|index| SealedId::packed(index, index))
        .collect()
}

#[test]
fn a_memo_survives_its_region_moving_and_absorbing() {
    let region = Region::new();
    let recorded = ids([3, 9, 27]);
    let written = region.set_memo(&recorded);
    assert!(written > 0, "the memo mints the region's first chunk");
    assert_eq!(region.memo(), Some(&recorded[..]));

    // The move a seal makes: the region leaves the slot and lands in the sealed cell, by value. The
    // bump moves; the chunk the memo sits in does not.
    let mut region = Box::new(region);
    assert_eq!(region.memo(), Some(&recorded[..]));

    // The move a merge makes: another region's bump joins the bundle. The memo is in this region's
    // own bump, which the absorb leaves where it is.
    let other = Region::new();
    other.writer().slice(&[7u64; 16]);
    let before = region.allocated_bytes();
    region.absorb(other);
    assert!(region.allocated_bytes() > before);
    assert_eq!(region.memo(), Some(&recorded[..]));

    // Written once: a second prime reports no bytes and leaves the first answer standing.
    assert_eq!(region.set_memo(&ids([1])), 0);
    assert_eq!(region.memo(), Some(&recorded[..]));
}

#[test]
fn an_empty_memo_reads_back_empty() {
    let region = Region::new();
    region.set_memo(&[]);
    assert_eq!(region.memo(), Some(&[][..]));
}
