//! The table's scratch region, as the verbs use it: reset at every verb's entry and never inside
//! one, and warm enough from construction that a verb's transients ask the allocator for nothing.
//!
//! These are the lib-test mirror of the harness's allocation criteria. The harness meters the
//! global allocator per verb; these read the region's own two figures — the chunk bytes it holds,
//! and the bytes handed out of them — directly.

use super::super::*;
use super::{Number, Owned, operand, pin, pinned};

/// Operands in the placement the dirtying helper makes. Wide enough that the per-operand lists
/// outgrow the region's first chunk, which is what leaves a reading behind: a bump rewinds the
/// last allocation when it is freed, so only a list that *grew* strands the buffer it came from.
const WIDE: usize = 256;

/// A placement of one value over `operands` copies of a source carrier, into another cell — the
/// widest transient a step builds, since the crossed-operand list and the views are both sized by
/// the operand count.
fn place_over(table: &mut CellTable<Owned>, from: Handle, into: Handle, operands: usize) {
    table
        .enter(from, |context| {
            let source = context.alloc::<Number>(|writer| writer.value(1));
            let carriers: Vec<_> = (0..operands).map(|_| operand(&source)).collect();
            context
                .alloc_into::<Number, Number>(into, &carriers, |writer, views| {
                    writer.value(views.iter().map(|view| *pinned(view)).sum())
                })
                .unwrap();
        })
        .unwrap();
}

/// A table whose region still holds the last verb's transients, and the two cells it was built
/// with. The assertion is the placement half of the criterion: a step's crossed operands and the
/// views its build closure received are **in the region**, so a wide enough placement is readable
/// in the region's own occupancy after the step returns.
fn dirtied() -> (CellTable<Owned>, Handle, Handle) {
    let mut table: CellTable<Owned> = CellTable::new(4, pin);
    let producer = table.create(None, None).unwrap();
    let consumer = table.create(None, None).unwrap();
    place_over(&mut table, producer, consumer, WIDE);
    assert!(
        table.scratch_at_rest().in_use() > 0,
        "a placement over {WIDE} operands left nothing in the region"
    );
    (table, producer, consumer)
}

/// The reset sits at each verb's entry as its own statement, so each of the three is checked
/// against a region the verb before it left occupied.
#[test]
fn a_create_clears_the_region_at_its_entry() {
    let (mut table, _, _) = dirtied();
    table.create(None, None).unwrap();
    assert_eq!(table.scratch_at_rest().in_use(), 0);
}

#[test]
fn an_enter_clears_the_region_at_its_entry() {
    let (mut table, producer, _) = dirtied();
    table.enter(producer, |_| ()).unwrap();
    assert_eq!(table.scratch_at_rest().in_use(), 0);
}

#[test]
fn a_release_clears_the_region_at_its_entry() {
    let (mut table, producer, consumer) = dirtied();
    // The consumer holds the producer's storage, so the release seals rather than reclaims and the
    // whole cascade runs — its own transients included.
    table
        .enter(consumer, |context| context.hold(producer).unwrap())
        .unwrap();
    table.release(producer, Absorption::Refused).unwrap();
    assert_eq!(table.scratch_at_rest().in_use(), 0);
}

/// A table warm from construction grows no chunk: the first round's transients fit the chunk the
/// constructor sized, and every round after it reuses the same bytes. This is the lib-test mirror
/// of the harness's "no verb's allocation count grows with the operand count" criterion — the
/// bytes a round asks for come out of a chunk that was paid at construction.
///
/// The round covers both verbs that build transients — a placement under `enter`, and a `release`
/// whose cascade seals a held producer — so the figure spans the placement's view list and the
/// disposal's nested worklists alike.
#[test]
fn a_warm_scratch_grows_no_chunk_across_repeated_verbs() {
    let mut table: CellTable<Owned> = CellTable::new(8, pin);
    let consumer = table.create(None, None).unwrap();

    let round = |table: &mut CellTable<Owned>| {
        let producer = table.create(None, None).unwrap();
        place_over(table, producer, consumer, 8);
        table
            .enter(consumer, |context| context.hold(producer).unwrap())
            .unwrap();
        table.release(producer, Absorption::Refused).unwrap();
    };

    round(&mut table);
    let warm = table.scratch_at_rest().capacity();
    round(&mut table);
    assert_eq!(
        table.scratch_at_rest().capacity(),
        warm,
        "a second round of the same verbs grew the region"
    );
    assert_eq!(
        warm,
        Scratch::new().capacity(),
        "a round outgrew the chunk the constructor sized"
    );
}
