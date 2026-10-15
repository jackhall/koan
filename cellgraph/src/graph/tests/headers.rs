//! The habitat header: one embedder value per habitat half, stored by the substrate and handed out
//! at that half's brand ([../../header.rs](../../header.rs)).
//!
//! What these pin: a region header laid in one step is read back by the cell's next step, by a
//! tenant of the cell and by a placement into it, and a placement lays one in a destination that
//! had none; a scratch header lasts while its bump does, holds no reset off, and one a delivery
//! lays is read by its consumer; a recycled region starts with none; a pinned and a copied operand
//! each carry their home's header, one homed in its destination carries none, and one redeemed
//! from sealed storage carries the sealed region's, before and after a `keep`; a splice and an
//! absorb keep the absorber's header; and a tail loop finds no header at any birth.
//!
//! The headers hold real borrows into region bytes — a [`Mark::Table`] is a `Cell` laid in the
//! half it describes, pointing at a number laid beside it — so Miri sees every read through a slot
//! and across a crossing as a read of the chunk the header names.

use std::cell::Cell;

use super::super::*;
use super::{Number, Owned, one, operand, operand_at, pin};

/// A header: a table in the half's own bytes, or a label borrowing storage outside the graph.
#[derive(Clone, Copy)]
pub(super) enum Mark<'graph, 'cell> {
    /// Invariant in `'cell` through the `Cell`, and a real borrow across the slot.
    Table(&'cell Cell<&'cell u32>),
    /// Laid without writing a byte.
    Label(&'graph str),
}

/// The header family.
pub(super) struct Marks;
crate::reattachable!(Marks => Mark<'graph, 'cell>);

/// The scratch state: a borrow of the cell's own scratch bump, so a parked state holds the bump.
pub(super) struct Held;
crate::reattachable!(both Held => &'scratch u32);

/// The delivered scratch family: a number a producer writes into the consumer's scratch habitat.
pub(super) struct Note;
crate::reattachable!(Note => &'cell u32);
impl DropFree for Note {}

/// The bundle: a note built in the consumer's scratch, a [`Number`] carrier filed at rest.
pub(super) struct Push;
impl<'graph> Delivery<'graph> for Push {
    type Scratch = Note;
    type Carrier = Number;
}

pub(super) type Graph = CellGraph<'static, Owned, Held, Push, 1, Marks>;
pub(super) type Context<'step, 'here, 'scratch, 'lent> =
    StepContext<'static, 'step, 'here, 'scratch, 'lent, Owned, Held, Push, 1, Marks>;

pub(super) fn new_graph(cap: u32) -> Graph {
    CellGraph::new(cap, pin)
}

/// A table over `value`, laid through `writer`: the number, then the `Cell` pointing at it.
pub(super) fn table<'cell>(writer: Writer<'cell, '_>, value: u32) -> Mark<'static, 'cell> {
    let number = one(writer, value);
    Mark::Table(one(writer, Cell::new(number)))
}

/// The number a table header points at; `None` for a label.
pub(super) fn value_of(mark: Mark<'_, '_>) -> Option<u32> {
    match mark {
        Mark::Table(table) => Some(*table.get()),
        Mark::Label(_) => None,
    }
}

/// The table's own borrow, at the header's brand.
pub(super) fn number_of<'cell>(mark: Mark<'_, 'cell>) -> &'cell u32 {
    match mark {
        Mark::Table(table) => table.get(),
        Mark::Label(_) => panic!("the header is a table"),
    }
}

/// Lay a table over `value` in the step's region header, or read back the one already there.
pub(super) fn lay(context: &Context<'_, '_, '_, '_>, value: u32) -> Option<u32> {
    value_of(
        context
            .header()
            .get_or_lay(|| table(context.writer(), value)),
    )
}

/// What the step's region header points at.
fn header_value(context: &Context<'_, '_, '_, '_>) -> Option<u32> {
    context.header().get().and_then(value_of)
}

/// A `Number` carrier homed where the step writes.
fn lifted<'step>(
    context: &Context<'step, '_, '_, '_>,
    value: u32,
) -> Ready<'static, 'step, Number> {
    context.lift::<Number>(one(context.writer(), value))
}

/// What a home header points at, at whichever brand the operand crossed.
fn home_value<'b>(home: HomeHeader<'static, 'b, Marks>) -> Option<u32> {
    home.read(|header, _| value_of(header.into_value()))
        .flatten()
}

/// What a crossed operand's home header points at.
fn crossed_home(view: &CrossedOperand<'static, '_, '_, Number, Marks>) -> Option<u32> {
    match view {
        CrossedOperand::Pinned { home, .. } => home_value(*home),
        CrossedOperand::Copied { home, .. } => home_value(*home),
    }
}

/// The table borrow a pinned operand's home header holds, at the destination's brand.
fn pinned_home<'cell>(view: &CrossedOperand<'static, 'cell, '_, Number, Marks>) -> &'cell u32 {
    let CrossedOperand::Pinned { home, .. } = view else {
        panic!("the operand was pinned")
    };
    home.read(|header, _| number_of(header.into_value()))
        .expect("the home has a header")
}

/// Build a number in `dest` and hand back its carrier.
fn number_into<'step>(
    context: &mut Context<'step, '_, '_, '_>,
    dest: impl Into<CellHandle>,
    value: u32,
) -> Ready<'static, 'step, Number> {
    context
        .alloc_into::<Number, Number>(dest, &[], |writer, _, _| Active::new(one(writer, value)))
        .unwrap()
}

#[test]
fn a_header_laid_in_a_step_comes_back_at_the_next_step() {
    let mut graph = new_graph(1);
    let cell = graph.create(None).unwrap();
    graph
        .enter(cell, |context| {
            assert!(context.header().get().is_none());
            assert_eq!(lay(context, 7), Some(7));
            assert_eq!(lay(context, 8), Some(7), "a laid header is never replaced");
        })
        .unwrap();
    let read = graph.enter(cell, |context| header_value(context)).unwrap();
    assert_eq!(read, Some(7));
}

#[test]
fn a_tenant_reads_its_hosts_header() {
    let mut graph = new_graph(1);
    let host = graph.create(None).unwrap();
    graph.enter(host, |context| lay(context, 7)).unwrap();
    let tenant = graph.create_tenant(host, None).unwrap();
    let read = graph
        .enter(tenant, |context| header_value(context))
        .unwrap();
    assert_eq!(read, Some(7));
    graph.release_tenant(tenant).unwrap();
}

#[test]
fn a_placement_reads_and_lays_its_destinations_header() {
    let mut graph = new_graph(3);
    let laid = graph.create(None).unwrap();
    let placer = graph.create(None).unwrap();
    let bare = graph.create(None).unwrap();
    graph.enter(laid, |context| lay(context, 7)).unwrap();

    let seen = graph
        .enter(placer, |context| {
            let mut seen = None;
            context
                .alloc_into::<Number, Number>(laid, &[], |writer, header, _| {
                    seen = header.get().and_then(value_of);
                    Active::new(one(writer, 0))
                })
                .unwrap();
            context
                .alloc_into::<Number, Number>(bare, &[], |writer, header, _| {
                    assert!(header.get().is_none());
                    Active::new(number_of(header.get_or_lay(|| table(writer, 9))))
                })
                .unwrap();
            seen
        })
        .unwrap();
    assert_eq!(seen, Some(7));
    let read = graph.enter(bare, |context| header_value(context)).unwrap();
    assert_eq!(read, Some(9));
}

#[test]
fn a_scratch_header_lasts_while_its_bump_does() {
    let mut graph = new_graph(1);
    let cell = graph.create(None).unwrap();
    graph
        .enter(cell, |context| {
            assert!(context.scratch_header().get().is_none());
            context
                .scratch_header()
                .get_or_lay(|| table(context.scratch_writer(), 3));
            context.store_scratch_state(one(context.scratch_writer(), 1));
        })
        .unwrap();
    // The state is taken and not parked again, so this step's end hands the bump back.
    let read = graph
        .enter(cell, |context| {
            assert_eq!(context.scratch_state().copied(), Some(1));
            context.scratch_header().get().and_then(value_of)
        })
        .unwrap();
    assert_eq!(read, Some(3));
    let cleared = graph
        .enter(cell, |context| {
            let cleared = context.scratch_header().get().is_none();
            // A header alone holds no reset off: nothing is parked over it.
            context
                .scratch_header()
                .get_or_lay(|| table(context.scratch_writer(), 4));
            cleared
        })
        .unwrap();
    assert!(cleared);
    let cleared = graph
        .enter(cell, |context| context.scratch_header().get().is_none())
        .unwrap();
    assert!(cleared);
}

#[test]
fn a_delivered_build_lays_its_consumers_scratch_header() {
    let mut graph = new_graph(2);
    let consumer = graph.create(None).unwrap();
    let producer = graph.create(None).unwrap();
    graph
        .enter(consumer, |context| context.register_receipts(1).unwrap())
        .unwrap();
    graph
        .enter(producer, |context| {
            context
                .deliver_scratch(consumer, 0, |writer, header, _| {
                    Active::new(number_of(header.get_or_lay(|| table(writer, 5))))
                })
                .unwrap();
        })
        .unwrap();
    let (header, note) = graph
        .enter(consumer, |context| {
            let header = context.scratch_header().get().and_then(value_of);
            let Ok(Receipt::Value(note)) = context.receipt(0) else {
                panic!("the slot holds the delivered note")
            };
            (header, *note)
        })
        .unwrap();
    assert_eq!((header, note), (Some(5), 5));
}

#[test]
fn a_recycled_region_starts_with_no_header() {
    let mut graph = new_graph(2);
    let written = graph.create(None).unwrap();
    let unwritten = graph.create(None).unwrap();
    graph.enter(written, |context| lay(context, 7)).unwrap();
    // A label writes no byte, so only the header marks the region as used.
    graph
        .enter(unwritten, |context| {
            context.header().get_or_lay(|| Mark::Label("label"));
        })
        .unwrap();
    assert_eq!(graph.region_bytes(unwritten).unwrap(), 0);
    graph
        .release(written, ReleaseAbsorption::IntoHolder)
        .unwrap();
    graph
        .release(unwritten, ReleaseAbsorption::IntoHolder)
        .unwrap();
    let born = [graph.create(None).unwrap(), graph.create(None).unwrap()];
    for cell in born {
        let empty = graph
            .enter(cell, |context| context.header().get().is_none())
            .unwrap();
        assert!(empty, "a new occupant finds no header");
    }

    let root = born[0];
    for mark in [Some(7), None] {
        let tree = graph.create_tree(root, None).unwrap();
        graph
            .enter(tree, |context| match mark {
                Some(value) => lay(context, value),
                None => value_of(context.header().get_or_lay(|| Mark::Label("label"))),
            })
            .unwrap();
        graph.release_tree(tree).unwrap();
        let next = graph.create_tree(root, None).unwrap();
        let empty = graph
            .enter(next, |context| context.header().get().is_none())
            .unwrap();
        assert!(empty, "a new tree cell finds no header");
        graph.release_tree(next).unwrap();
    }
}

#[test]
fn a_pinned_and_a_copied_operand_carry_their_homes_header() {
    // Keyed on the stated copy cost, so one placement pins one operand and copies the other.
    let mut graph: Graph = CellGraph::new(4, |prices: Prices| match prices.copy_bytes {
        0 => Verdict::Copy,
        _ => Verdict::Pin,
    });
    let first = graph.create(None).unwrap();
    let second = graph.create(None).unwrap();
    let placer = graph.create(None).unwrap();
    let dest = graph.create(None).unwrap();
    graph.enter(first, |context| lay(context, 1)).unwrap();
    graph.enter(second, |context| lay(context, 2)).unwrap();

    let (copied, kept) = graph
        .enter(placer, |context| {
            let pinned = number_into(context, first, 10);
            let copy = number_into(context, second, 20);
            let mut copied = None;
            let placed = context
                .alloc_into::<Number, Number>(
                    dest,
                    &[operand_at(&pinned, usize::MAX), operand_at(&copy, 0)],
                    |_, _, views| {
                        copied = crossed_home(&views[1]);
                        // The pinned home's header borrow, embedded in the destination's value.
                        Active::new(pinned_home(&views[0]))
                    },
                )
                .unwrap();
            (copied, context.keep(placed))
        })
        .unwrap();
    assert_eq!(copied, Some(2));

    // The pin is what keeps the first home's bytes, header included, past its release.
    graph.release(first, ReleaseAbsorption::Refused).unwrap();
    let read = graph
        .enter(dest, |context| {
            *context
                .read(&context.redeem(kept).expect("the destination is the home"))
                .value()
        })
        .unwrap();
    assert_eq!(read, 1);
}

#[test]
fn an_operand_homed_in_its_destination_carries_no_header() {
    let mut graph = new_graph(2);
    let home = graph.create(None).unwrap();
    let placer = graph.create(None).unwrap();
    graph.enter(home, |context| lay(context, 1)).unwrap();
    graph
        .enter(placer, |context| {
            let value = number_into(context, home, 10);
            context
                .alloc_into::<Number, Number>(home, &[operand(&value)], |writer, header, views| {
                    assert_eq!(crossed_home(&views[0]), None);
                    // The destination's header comes through the build's own door instead.
                    assert_eq!(header.get().and_then(value_of), Some(1));
                    Active::new(one(writer, 0))
                })
                .unwrap();

            assert_eq!(lay(context, 3), Some(3));
            let own = lifted(context, 4);
            context.alloc_here(&[operand(&own)], |_, header, views| {
                assert_eq!(crossed_home(&views[0]), None);
                assert_eq!(header.get().and_then(value_of), Some(3));
            });
        })
        .unwrap();
}

#[test]
fn an_operand_redeemed_from_sealed_storage_carries_the_sealed_header() {
    let mut graph = new_graph(3);
    let home = graph.create(None).unwrap();
    let redeemer = graph.create(None).unwrap();
    let dest = graph.create(None).unwrap();
    graph.enter(home, |context| lay(context, 1)).unwrap();
    // The redeemer holds the home, so the home seals at its release; its kept value is homed
    // there.
    let kept = graph
        .enter(redeemer, |context| {
            context.hold(home).unwrap();
            let value = number_into(context, home, 10);
            context.keep(value)
        })
        .unwrap();
    graph.release(home, ReleaseAbsorption::Refused).unwrap();
    assert_eq!(graph.cells.sealed.len(), 1);

    // Placed straight out of the redeem, and again after a `keep` of the redeemed carrier: both
    // name the redeemer as home and the sealed cell alone as reach.
    let (first, again) = graph
        .enter(redeemer, |context| {
            let carrier = context
                .redeem(kept)
                .expect("the redeemer holds the sealed cell");
            let first = context
                .alloc_into::<Number, Number>(dest, &[operand(&carrier)], |_, _, views| {
                    Active::new(pinned_home(&views[0]))
                })
                .unwrap();
            (context.keep(first), context.keep(carrier))
        })
        .unwrap();
    let second = graph
        .enter(redeemer, |context| {
            let carrier = context.redeem(again).expect("its own dormant carrier");
            let placed = context
                .alloc_into::<Number, Number>(dest, &[operand(&carrier)], |_, _, views| {
                    Active::new(pinned_home(&views[0]))
                })
                .unwrap();
            context.keep(placed)
        })
        .unwrap();

    // The destination's holds keep the sealed storage once the redeemer is gone.
    graph
        .release(redeemer, ReleaseAbsorption::IntoHolder)
        .unwrap();
    let read = graph
        .enter(dest, |context| {
            let first = *context.read(&context.redeem(first).unwrap()).value();
            let second = *context.read(&context.redeem(second).unwrap()).value();
            (first, second)
        })
        .unwrap();
    assert_eq!(read, (1, 1));
}

#[test]
fn a_splice_and_an_absorb_keep_the_absorbers_header() {
    // A slab absorb: the absorbed cell's header is discarded, and a carrier homed there crosses
    // with the absorber's.
    let mut graph = new_graph(3);
    let absorbed = graph.create(None).unwrap();
    let absorber = graph.create(None).unwrap();
    let dest = graph.create(None).unwrap();
    graph.enter(absorbed, |context| lay(context, 1)).unwrap();
    let kept = graph
        .enter(absorber, |context| {
            assert_eq!(lay(context, 2), Some(2));
            context.hold(absorbed).unwrap();
            let value = number_into(context, absorbed, 10);
            context.keep(value)
        })
        .unwrap();
    graph
        .release(absorbed, ReleaseAbsorption::IntoHolder)
        .unwrap();
    assert_eq!(graph.cells.merges.into_cell, 1);
    let (own, crossed) = graph
        .enter(absorber, |context| {
            let carrier = context.redeem(kept).expect("the absorber answers");
            let mut crossed = None;
            context
                .alloc_into::<Number, Number>(dest, &[operand(&carrier)], |writer, _, views| {
                    crossed = crossed_home(&views[0]);
                    Active::new(one(writer, 0))
                })
                .unwrap();
            (header_value(context), crossed)
        })
        .unwrap();
    assert_eq!((own, crossed), (Some(2), Some(2)));

    // An unwritten absorber takes the source over whole and keeps its own header.
    let mut graph = new_graph(2);
    let absorbed = graph.create(None).unwrap();
    let absorber = graph.create(None).unwrap();
    graph.enter(absorbed, |context| lay(context, 1)).unwrap();
    graph
        .enter(absorber, |context| {
            context.header().get_or_lay(|| Mark::Label("absorber"));
            context.hold(absorbed).unwrap();
        })
        .unwrap();
    graph
        .release(absorbed, ReleaseAbsorption::IntoHolder)
        .unwrap();
    assert_eq!(graph.cells.merges.into_cell, 1);
    assert!(
        graph.region_bytes(absorber).unwrap() > 0,
        "the source's bytes moved in"
    );
    let label = graph
        .enter(absorber, |context| match context.header().get() {
            Some(Mark::Label(label)) => label,
            _ => panic!("the absorber's label survives the takeover"),
        })
        .unwrap();
    assert_eq!(label, "absorber");

    // A tree splice: a child pledged by an upward pin into its root splices into it at release.
    let mut graph = new_graph(1);
    let root = graph.create(None).unwrap();
    let child = graph.create_tree(root, None).unwrap();
    graph.enter(root, |context| lay(context, 1)).unwrap();
    graph
        .enter(child, |context| {
            assert_eq!(lay(context, 2), Some(2));
            let value = lifted(context, 20);
            context
                .alloc_into::<Number, Number>(
                    root,
                    &[operand_at(&value, usize::MAX)],
                    |_, _, views| Active::new(pinned_home(&views[0])),
                )
                .unwrap();
        })
        .unwrap();
    let child_bytes = graph.regions.tree_bytes(child.index());
    graph.release_tree(child).unwrap();
    assert!(
        graph.region_bytes(root).unwrap() > child_bytes,
        "the child spliced in"
    );
    let read = graph.enter(root, |context| header_value(context)).unwrap();
    assert_eq!(read, Some(1));
}

/// Where a table header's number landed, as a number that can leave its step.
fn address_of(mark: Mark<'_, '_>) -> usize {
    std::ptr::from_ref(number_of(mark)) as usize
}

#[test]
fn a_tail_loop_lays_a_header_on_every_hop() {
    let mut graph = new_graph(2);
    let mut current = graph.create(None).unwrap();
    let mut addresses = vec![
        graph
            .enter(current, |context| {
                address_of(context.header().get_or_lay(|| table(context.writer(), 0)))
            })
            .unwrap(),
    ];
    for hop in 1..=64 {
        let next = graph.create(None).unwrap();
        let laid = graph
            .enter(next, |context| {
                assert!(
                    context.header().get().is_none(),
                    "hop {hop}: a birth finds none"
                );
                address_of(context.header().get_or_lay(|| table(context.writer(), hop)))
            })
            .unwrap();
        graph
            .release(current, ReleaseAbsorption::IntoHolder)
            .unwrap();
        addresses.push(laid);
        current = next;
    }
    // Each birth draws the chunk the release before it freed — the cell born two hops back — and
    // lays its header in the same place. Miri recycles nothing, so the check is off there.
    if !cfg!(miri) {
        for window in addresses.windows(3) {
            assert_eq!(window[2], window[0]);
        }
    }
}

/// A tree cell writes its lender's scratch through the lent doors, and the lender reads it back at
/// its own `'scratch` once it steps; the bump outlives both steps while the borrower lives, and
/// goes at the first quiet step end after its death.
#[test]
fn a_tree_cell_lays_a_header_in_its_lenders_scratch_and_the_lender_reads_it() {
    let mut graph = new_graph(1);
    let root = graph.create(None).unwrap();
    let child = graph.create_tree(root, None).unwrap();
    let home = CellHome::Slab(root.slot());
    assert!(
        graph
            .enter(root, |context| context.lent_scratch().is_none())
            .unwrap()
    );

    graph
        .enter(child, |context| {
            let (writer, header) = context.lent_scratch().expect("a tree cell has a lender");
            value_of(header.get_or_lay(|| table(writer, 41)))
        })
        .unwrap();
    assert!(
        graph.regions.scratch_in_use(home) > 0,
        "the lent bump kept its bytes"
    );
    let read = graph
        .enter(root, |context| {
            context.scratch_header().get().and_then(value_of)
        })
        .unwrap();
    assert_eq!(read, Some(41));
    assert!(
        graph.regions.scratch_in_use(home) > 0,
        "the borrower still holds it"
    );

    graph.release_tree(child).unwrap();
    graph.enter(root, |_| ()).unwrap();
    assert_eq!(graph.regions.scratch_in_use(home), 0);
    let read = graph
        .enter(root, |context| context.scratch_header().get().is_none())
        .unwrap();
    assert!(read, "the reset cleared the header");

    graph.release(root, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(graph.is_empty());
}

/// A header whose form borrows plain bytes of its own half, with no `Cell` around them.
struct Plain;
crate::reattachable!(Plain => &'cell u32);

/// A region whose header borrows its own frozen bytes goes back to the allocator with the header
/// still laid, from a tree cell's disposal and from a slab cell's release.
#[test]
fn a_region_whose_header_borrows_its_plain_bytes_retires() {
    let mut graph: CellGraph<'static, Owned, NoScratch, NoDelivery, 1, Plain> =
        CellGraph::new(2, pin);
    let root = graph.create(None).expect("a slot is free");
    let tree = graph.create_tree(root, None).expect("the root is live");
    let lay = |context: &mut StepContext<
        'static,
        '_,
        '_,
        '_,
        '_,
        Owned,
        NoScratch,
        NoDelivery,
        1,
        Plain,
    >| {
        let writer = context.writer();
        *context.header().get_or_lay(|| one(writer, 7))
    };
    assert_eq!(graph.enter(tree, lay), Ok(7));
    assert_eq!(graph.enter(root, lay), Ok(7));
    graph.release_tree(tree).expect("the tree cell releases");
    graph
        .release(root, ReleaseAbsorption::IntoHolder)
        .expect("the root releases");
    assert!(graph.is_empty());
}
