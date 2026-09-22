//! The memo's pointer stability, and what the writer's producer-decided runs cost the region.
//!
//! `BumpRun` is the crate's second use of the argument `Dormant` and `redeem` already stand on — a
//! bump allocates in pointer-stable chunks, a `Bump` moves without moving a chunk byte, and a
//! region's chunks are reset or freed only once the region itself is gone. These run under Miri, where a
//! stale pointer or a leaked chunk is a failure rather than a coincidence.
//!
//! `Run` and `Prose` are measured rather than only exercised: the pair of growth tests reads the
//! region's handed-out bytes to show that a run left alone grows by its delta, and that one with a
//! write interleaved between its pushes strands what it outgrows. Both are what the doors promise.
//!
//! `ThinRun` is the one raw layout the crate builds by hand — a header and a run in one allocation —
//! so its tests pin the offset against std's own layout arithmetic and read back every shape the
//! arithmetic has an edge at: empty, zero-sized, over-aligned, interleaved and outgrown.

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
    assert!(
        region.absorb(other).is_none(),
        "a written bump joins the bundle"
    );
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

#[test]
fn a_thin_run_reads_back_what_the_fill_wrote() {
    let region = Region::new();
    let run = region.writer().thin_run(5, |index| index as u64 * 3);
    let copy = run;
    assert_eq!(run.len(), 5);
    assert!(!run.is_empty());
    assert_eq!(copy.as_slice(), &[0, 3, 6, 9, 12]);
}

#[test]
fn a_thin_run_handle_is_one_pointer_wide() {
    assert_eq!(size_of::<ThinRun<'static, u64>>(), size_of::<usize>());
    assert_eq!(
        size_of::<Option<ThinRun<'static, u8>>>(),
        size_of::<usize>()
    );
}

#[repr(align(16))]
#[derive(Debug, PartialEq)]
struct Wide(u8);

#[test]
fn a_thin_runs_offset_matches_the_layout_std_would_extend() {
    fn std_offset<T>() -> usize {
        let (_, offset) = Layout::new::<ThinHeader>()
            .extend(Layout::new::<T>())
            .expect("small layouts extend");
        offset
    }
    assert_eq!(ThinRun::<u8>::OFFSET, std_offset::<u8>());
    assert_eq!(ThinRun::<u64>::OFFSET, std_offset::<u64>());
    assert_eq!(ThinRun::<Wide>::OFFSET, std_offset::<Wide>());
}

#[test]
fn a_thin_run_of_length_zero_is_empty() {
    let region = Region::new();
    let run = region
        .writer()
        .thin_run(0, |_| -> u32 { unreachable!("nothing to fill") });
    assert!(run.is_empty());
    assert_eq!(run.as_slice(), &[] as &[u32]);
}

#[test]
fn a_thin_run_of_zero_sized_payloads_reads_every_unit() {
    let region = Region::new();
    let mut calls = 0;
    let run = region.writer().thin_run(7, |_| calls += 1);
    assert_eq!(calls, 7);
    assert_eq!(run.len(), 7);
    assert_eq!(run.as_slice().len(), 7);
}

#[test]
fn a_thin_run_of_over_aligned_payloads_lands_them_aligned() {
    let region = Region::new();
    // An odd-sized write first, so the bump's cursor is off any 16-byte boundary.
    region.writer().fill(3, |_| 1u8);
    let run = region.writer().thin_run(3, |index| Wide(index as u8));
    for (index, element) in run.as_slice().iter().enumerate() {
        assert_eq!((element as *const Wide).addr() % 16, 0);
        assert_eq!(element, &Wide(index as u8));
    }
}

#[test]
fn a_thin_run_whose_fill_writes_into_the_same_region() {
    let region = Region::new();
    let writer = region.writer();
    let run = writer.thin_run(4, |index| {
        writer.fill(index + 1, |inner| (index * 10 + inner) as u32)
    });
    assert_eq!(run.len(), 4);
    for (index, sub) in run.as_slice().iter().enumerate() {
        let expected: Vec<u32> = (0..=index)
            .map(|inner| (index * 10 + inner) as u32)
            .collect();
        assert_eq!(*sub, &expected[..]);
    }
}

#[test]
fn a_thin_run_survives_the_region_growing_under_it() {
    let region = Region::new();
    let writer = region.writer();
    let run = writer.thin_run(8, |index| index as u64);
    let chunks = region.allocated_bytes();
    // Enough to force a new chunk past the one holding the run.
    let mut rounds = 0;
    while region.allocated_bytes() == chunks {
        writer.fill(512, |_| 0u64);
        rounds += 1;
    }
    assert!(rounds > 0);
    assert_eq!(run.len(), 8);
    assert_eq!(run.as_slice(), &[0, 1, 2, 3, 4, 5, 6, 7]);
}

#[test]
fn a_reset_bump_reads_unwritten() {
    let mut region = Region::new();
    assert!(region.is_unwritten(), "a cold bump has handed nothing out");
    region.writer().fill(16, |_| 7u64);
    assert!(!region.is_unwritten());
    region.bump.reset();
    assert!(region.bump.allocated_bytes() > 0, "a reset keeps the chunk");
    assert!(region.is_unwritten(), "and hands back every byte of it");
    region.writer().fill(1, |_| 7u8);
    assert!(!region.is_unwritten());
}
