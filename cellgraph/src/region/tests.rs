//! The memo's pointer stability, and what the writer's producer-decided runs cost the region.
//!
//! `BumpRun` is the crate's second use of the argument `Dormant` and `redeem` already stand on — a
//! bump allocates in pointer-stable chunks, a `Bump` moves without moving a chunk byte, and a
//! region never resets and frees its chunks only at its own drop. These run under Miri, where a
//! stale pointer or a leaked chunk is a failure rather than a coincidence.
//!
//! `Run` and `Prose` are measured rather than only exercised: the pair of growth tests reads the
//! region's handed-out bytes to show that a run left alone grows by its delta, and that one with a
//! write interleaved between its pushes strands what it outgrows. Both are what the doors promise.

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
    other.writer().fill(16, |_| 7u64);
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

/// Bytes the region's own bump has handed out, stranded buffers included — chunk capacity less
/// what is still free in the newest chunk. What separates a run that grew in place from one that
/// left a buffer behind at every growth.
fn handed_out(region: &Region) -> usize {
    region.bump.allocated_bytes() - region.bump.chunk_capacity()
}

#[test]
fn a_run_reads_back_in_push_order() {
    let region = Region::new();
    let mut run = region.writer().run();
    assert!(run.is_empty());
    for value in 0..5u32 {
        run.push(value * 2);
    }
    run.extend([100u32, 200]);
    assert_eq!(run.len(), 7);
    assert_eq!(run.finish(), &[0, 2, 4, 6, 8, 100, 200]);

    // A run nothing was pushed onto costs the region nothing and reads back empty.
    let before = region.allocated_bytes();
    assert_eq!(region.writer().run::<u32>().finish(), &[] as &[u32]);
    assert_eq!(region.allocated_bytes(), before);
}

#[test]
fn a_run_grows_in_place_while_it_is_the_regions_newest_allocation() {
    let region = Region::new();
    // Warm the region into a chunk with room to spare, so the growth below is measured against
    // free bytes rather than against bumpalo's chunk sizing.
    region.writer().fill(4096, |_| 0u64);
    let chunks = region.allocated_bytes();
    let before = handed_out(&region);

    let mut run = region.writer().run();
    for value in 0..64u64 {
        run.push(value);
    }
    let written = run.finish();

    assert_eq!(written.len(), 64);
    assert_eq!(
        region.allocated_bytes(),
        chunks,
        "the run fit the warm chunk"
    );
    // Every growth allocated only the delta and slid the bytes down, so the run cost its own bytes
    // and not one more: no buffer was outgrown and left behind.
    assert_eq!(handed_out(&region) - before, 64 * size_of::<u64>());
}

#[test]
fn an_allocation_between_pushes_strands_the_buffer_the_run_outgrows() {
    let region = Region::new();
    region.writer().fill(4096, |_| 0u64);
    let chunks = region.allocated_bytes();
    let before = handed_out(&region);

    let writer = region.writer();
    let mut run = writer.run();
    run.push(0u64);
    // A write into the same region between pushes: the run is no longer the newest allocation, so
    // every growth from here copies to a fresh buffer and abandons the old one.
    let between = writer.fill(4, |index| index as u64);
    for value in 1..64u64 {
        run.push(value);
    }
    let written = run.finish();

    assert_eq!(written.len(), 64);
    assert_eq!(between, &[0, 1, 2, 3]);
    assert_eq!(
        region.allocated_bytes(),
        chunks,
        "the run fit the warm chunk"
    );
    // The cost is not a number worth pinning, only that it is above what the contents occupy: the
    // abandoned buffers stay dead region bytes until the region dies.
    assert!(handed_out(&region) - before > 64 * size_of::<u64>() + 4 * size_of::<u64>());
}

#[test]
fn prose_takes_formatted_writes_and_multi_byte_text() {
    use std::fmt::Write;

    let region = Region::new();
    let mut prose = region.writer().prose();
    assert!(prose.is_empty());
    write!(prose, "{} zen ", 42).unwrap();
    prose.write_str("庭園").unwrap();
    assert_eq!(prose.len(), "42 zen ".len() + "庭園".len());
    assert_eq!(prose.finish(), "42 zen 庭園");

    assert_eq!(region.writer().prose().finish(), "");
}

#[test]
fn a_run_abandoned_mid_build_leaves_the_region_writable() {
    let region = Region::new();
    {
        let mut run = region.writer().run();
        run.extend(0..32u64);
    }
    // Nothing ran a destructor and nothing is poisoned: the bump takes the next write as usual.
    assert_eq!(
        region.writer().fill(3, |index| index as u64 + 1),
        &[1, 2, 3]
    );
    assert_eq!(region.writer().text("still writable"), "still writable");
}
