//! Region recycling: a reclaimed region's chunks wait on a bounded last-in-first-out spare list
//! that evicts its oldest when full, and the next birth draws them, so a release-then-create loop stays off the allocator.
//!
//! What these pin: every reclaim path feeds the list — a slab reclaim, an unpledged tree disposal,
//! a sealed cell's retirement, and every bump of an absorber's bundle; a birth draws the newest
//! spare; a bump nothing was written into is handed back rather than bundled or billed; and the
//! list stays under a proportion of the recent live-cell count.
//!
//! The whole module is off under Miri, where recycling is: a reclaimed chunk goes back to the
//! allocator there, so that a use after the reclaim is an error Miri can see.

use super::super::*;
use super::{Owned, one, pin};

/// A graph whose spare list is bounded well above anything these shapes retire, over an average
/// that settles within a birth or two, so a test about reuse is not also a test about the bound.
fn roomy(cap: u32) -> CellGraph<'static, Owned> {
    CellGraph::with_config(
        Config {
            cap,
            spare_proportion: 8,
            spare_window_shift: 1,
        },
        pin,
    )
}

/// Write one value into the cell and report where it landed. An address leaves a step as a
/// number, which is all a reuse assertion compares.
fn write_in(graph: &mut CellGraph<'static, Owned>, cell: impl Into<CellHandle>) -> usize {
    graph
        .enter(cell, |context| {
            std::ptr::from_ref(one(context.writer(), 7u64)) as usize
        })
        .unwrap()
}

#[test]
fn a_released_cells_chunk_serves_the_next_create() {
    let mut graph = roomy(2);
    let first = graph.create(None).unwrap();
    let written = write_in(&mut graph, first);
    graph.release(first, ReleaseAbsorption::IntoHolder).unwrap();
    assert_eq!(graph.regions.spare_len(), 1);

    let second = graph.create(None).unwrap();
    assert_eq!(graph.regions.spare_len(), 0);
    assert_eq!(write_in(&mut graph, second), written);
    graph
        .release(second, ReleaseAbsorption::IntoHolder)
        .unwrap();
    assert!(graph.is_empty());
}

#[test]
fn the_spare_list_is_last_in_first_out() {
    let mut graph = roomy(2);
    let older = graph.create(None).unwrap();
    let newer = graph.create(None).unwrap();
    let older_written = write_in(&mut graph, older);
    let newer_written = write_in(&mut graph, newer);
    graph.release(older, ReleaseAbsorption::IntoHolder).unwrap();
    graph.release(newer, ReleaseAbsorption::IntoHolder).unwrap();

    let first_born = graph.create(None).unwrap();
    let second_born = graph.create(None).unwrap();
    assert_eq!(write_in(&mut graph, first_born), newer_written);
    assert_eq!(write_in(&mut graph, second_born), older_written);
}

#[test]
fn a_full_spare_list_evicts_its_oldest() {
    // One spare per live cell, averaged over no window, so the bound is the live count exactly.
    let mut graph: CellGraph<'static, Owned> = CellGraph::with_config(
        Config {
            cap: 4,
            spare_proportion: 1,
            spare_window_shift: 0,
        },
        pin,
    );
    let _keeper = graph.create(None).unwrap();
    let oldest = graph.create(None).unwrap();
    let middle = graph.create(None).unwrap();
    let newest = graph.create(None).unwrap();
    write_in(&mut graph, oldest);
    let middle_written = write_in(&mut graph, middle);
    let newest_written = write_in(&mut graph, newest);
    graph
        .release(oldest, ReleaseAbsorption::IntoHolder)
        .unwrap();
    graph
        .release(middle, ReleaseAbsorption::IntoHolder)
        .unwrap();
    assert_eq!(graph.regions.spare_len(), 2);
    // Retired while two cells are live, the keeper and itself, onto a list already two long: the
    // oldest spare goes.
    graph
        .release(newest, ReleaseAbsorption::IntoHolder)
        .unwrap();
    assert_eq!(graph.regions.spare_len(), 2);

    let first_born = graph.create(None).unwrap();
    let second_born = graph.create(None).unwrap();
    assert_eq!(graph.regions.spare_len(), 0);
    assert_eq!(write_in(&mut graph, first_born), newest_written);
    assert_eq!(write_in(&mut graph, second_born), middle_written);
}

#[test]
fn an_unpledged_tree_cells_chunk_is_recycled() {
    let mut graph = roomy(1);
    let root = graph.create(None).unwrap();
    let tree = graph.create_tree(root, None).unwrap();
    let written = write_in(&mut graph, tree);
    graph.release_tree(tree).unwrap();
    assert_eq!(graph.regions.spare_len(), 1);

    let next = graph.create_tree(root, None).unwrap();
    assert_eq!(graph.regions.spare_len(), 0);
    assert_eq!(write_in(&mut graph, next), written);
}

#[test]
fn a_retired_sealed_cells_chunks_are_recycled() {
    let mut graph = roomy(2);
    let producer = graph.create(None).unwrap();
    let consumer = graph.create(None).unwrap();
    write_in(&mut graph, producer);
    graph
        .enter(consumer, |context| context.hold(producer))
        .unwrap()
        .unwrap();
    // Refused, so the producer seals rather than folding into its one holder.
    graph.release(producer, ReleaseAbsorption::Refused).unwrap();
    assert_eq!(graph.cells.sealed.len(), 1);
    assert_eq!(graph.regions.spare_len(), 0, "a seal retains its chunks");

    // The consumer never wrote, so the one bump that comes back is the sealed cell's.
    graph
        .release(consumer, ReleaseAbsorption::IntoHolder)
        .unwrap();
    assert!(graph.is_empty());
    assert_eq!(graph.regions.spare_len(), 1);
}

#[test]
fn an_absorbers_reclaim_returns_every_bump_it_took_in() {
    let mut graph = roomy(2);
    let producer = graph.create(None).unwrap();
    let consumer = graph.create(None).unwrap();
    write_in(&mut graph, producer);
    write_in(&mut graph, consumer);
    graph
        .enter(consumer, |context| context.hold(producer))
        .unwrap()
        .unwrap();
    graph
        .release(producer, ReleaseAbsorption::IntoHolder)
        .unwrap();
    assert_eq!(graph.cells.merges.into_cell, 1);
    assert_eq!(graph.regions.spare_len(), 0, "a splice keeps the chunk");

    graph
        .release(consumer, ReleaseAbsorption::IntoHolder)
        .unwrap();
    assert_eq!(graph.regions.spare_len(), 2);
}

#[test]
fn a_warm_unwritten_absorber_takes_the_source_over_whole() {
    let mut graph = roomy(2);
    let seed = graph.create(None).unwrap();
    write_in(&mut graph, seed);
    graph.release(seed, ReleaseAbsorption::IntoHolder).unwrap();

    // The consumer draws the seed's chunk and writes nothing; the producer is born cold and
    // claims a chunk of its own.
    let consumer = graph.create(None).unwrap();
    let producer = graph.create(None).unwrap();
    assert_eq!(graph.regions.spare_len(), 0);
    write_in(&mut graph, producer);
    let produced = graph.region_bytes(producer).unwrap();
    graph
        .enter(consumer, |context| context.hold(producer))
        .unwrap()
        .unwrap();
    graph
        .release(producer, ReleaseAbsorption::IntoHolder)
        .unwrap();

    // The warm bump holds no value, so it goes back to the list rather than into the bundle, and
    // the bundle is billed for the producer's chunk alone.
    assert_eq!(graph.regions.spare_len(), 1);
    assert_eq!(graph.region_bytes(consumer).unwrap(), produced);
    graph
        .release(consumer, ReleaseAbsorption::IntoHolder)
        .unwrap();
    assert_eq!(graph.regions.spare_len(), 2);
}

#[test]
fn a_cell_that_never_writes_gives_its_drawn_bump_back() {
    let mut graph = roomy(1);
    let seed = graph.create(None).unwrap();
    write_in(&mut graph, seed);
    graph.release(seed, ReleaseAbsorption::IntoHolder).unwrap();

    let idle = graph.create(None).unwrap();
    assert_eq!(graph.regions.spare_len(), 0);
    graph.release(idle, ReleaseAbsorption::IntoHolder).unwrap();
    assert_eq!(graph.regions.spare_len(), 1);
}

/// One hop of a two-cell tail loop: the successor is born, then the predecessor dies.
fn hop(graph: &mut CellGraph<'static, Owned>, current: SlabHandle) -> SlabHandle {
    let next = graph.create(None).unwrap();
    write_in(graph, next);
    graph
        .release(current, ReleaseAbsorption::IntoHolder)
        .unwrap();
    next
}

#[test]
fn the_spare_list_is_bounded_by_recent_live_cells() {
    const DEPTH: usize = 32;
    let mut graph: CellGraph<'static, Owned> = CellGraph::with_config(
        Config {
            cap: 2,
            spare_proportion: 2,
            spare_window_shift: 2,
        },
        pin,
    );
    let root = graph.create(None).unwrap();
    let mut chain = Vec::new();
    let mut parent = CellHandle::Slab(root);
    for _ in 0..DEPTH {
        let tree = graph.create_tree(parent, None).unwrap();
        write_in(&mut graph, tree);
        chain.push(tree);
        parent = CellHandle::Tree(tree);
    }
    // The unwind: every cell disposes at its own release, leaf first, and retires its chunk.
    for tree in chain.into_iter().rev() {
        graph.release_tree(tree).unwrap();
    }
    let after_unwind = graph.regions.spare_len();
    assert!(
        after_unwind < DEPTH,
        "the bound fell with the live count and evicted bumps, leaving {after_unwind}"
    );
    assert!(after_unwind > 4, "a deep peak leaves a long list behind");

    // A two-cell loop draws one and retires one per hop, and the first retire evicts the whole
    // excess a peak left behind.
    let mut current = root;
    for _ in 0..4 * DEPTH {
        let before = graph.regions.spare_len();
        let next = graph.create(None).unwrap();
        assert_eq!(
            graph.regions.spare_len(),
            before - 1,
            "the loop still draws"
        );
        write_in(&mut graph, next);
        graph
            .release(current, ReleaseAbsorption::IntoHolder)
            .unwrap();
        current = next;
    }
    // The bound is read where a bump is pushed, which in this loop is with both cells live: a
    // proportion of two over an average of at most two.
    assert!(graph.regions.spare_bound() <= 4);
    assert!(graph.regions.spare_len() <= 4);
    assert!(graph.regions.spare_len() >= 1);
}

#[test]
fn a_lone_looping_cell_keeps_one_spare() {
    let mut graph: CellGraph<'static, Owned> = CellGraph::new(2, pin);
    let mut current = graph.create(None).unwrap();
    write_in(&mut graph, current);
    for _ in 0..256 {
        current = hop(&mut graph, current);
        assert!(
            graph.regions.spare_len() >= 1,
            "the predecessor's chunk waits"
        );
    }
}

#[test]
fn a_zero_proportion_recycles_nothing() {
    let mut graph: CellGraph<'static, Owned> = CellGraph::with_config(
        Config {
            spare_proportion: 0,
            ..Config::new(2)
        },
        pin,
    );
    let mut current = graph.create(None).unwrap();
    write_in(&mut graph, current);
    for _ in 0..8 {
        current = hop(&mut graph, current);
        assert_eq!(graph.regions.spare_len(), 0);
    }
}

#[test]
#[should_panic(expected = "spare window shift")]
fn a_window_too_wide_to_move_is_refused() {
    let _: CellGraph<'static, Owned> = CellGraph::with_config(
        Config {
            spare_window_shift: 24,
            ..Config::new(2)
        },
        pin,
    );
}
