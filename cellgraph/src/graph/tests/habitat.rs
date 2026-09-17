//! The scratch habitat: a second bump per cell at the `'scratch` brand, a second continuation slot
//! over its own family that carries a scratch structure across a park, and a reset at `enter`
//! whenever that slot is empty. Named apart from [`scratch`](super::scratch), which is about the
//! graph's one verb-transient region.
//!
//! What these pin: a scratch structure — an invariant one, written through after its re-anchor —
//! survives parks and goes once unnamed, in both region-owning kinds; an absorb moves a region and
//! leaves the absorber's scratch where it is; a departing cell's scratch goes at its disposal and
//! into no seal; scratch bytes are in no price; and the two doors are plain moves of the slot,
//! including under a panic.
//!
//! And for a shared region: a tenant's scratch is its host's bump, whose reset waits on every
//! tenant's scratch slot as well as the host's own — until a tenant dies, or the host does.

use std::cell::Cell;
use std::panic::{AssertUnwindSafe, catch_unwind};

use super::super::*;
use super::{Number, number_here, operand_at, pin};

/// The storage half: a run in the cell's region.
struct Storage;
crate::reattachable!(Storage => &'cell [u32]);

/// The scratch half. Invariant in `'cell` through the `Cell`, so the re-anchor at a fresh
/// `'scratch` each step is exercised in the shape an embedder's slot table has.
#[derive(Clone, Copy)]
struct Parked<'cell> {
    spine: &'cell [&'cell u32],
    slot: &'cell Cell<&'cell u32>,
}
struct Spine;
crate::reattachable!(Spine => Parked<'cell>);

type Graph = CellGraph<'static, Storage, Spine>;

/// Scratch bytes in use in the bump a step in `cell` writes: its own, or its host's.
fn scratch_in_use<S: Reattachable<'static>>(
    graph: &CellGraph<'static, Storage, S>,
    cell: impl Into<CellHandle>,
) -> usize {
    let home = graph
        .cells
        .write_home(cell.into())
        .expect("the cell is live");
    graph.regions.scratch_in_use(home)
}

fn round_trip(graph: &mut Graph, cell: CellHandle) {
    // Step one: storage, then scratch that embeds borrows of it.
    graph
        .enter(cell, |context| {
            let numbers = context.writer().fill(3, |index| index as u32 + 10);
            let spine = context
                .scratch_writer()
                .fill(2, |index| &numbers[index + 1]);
            let slot = &context.scratch_writer().fill(1, |_| Cell::new(&numbers[0]))[0];
            context.store_successor(numbers);
            context.store_scratch_successor(Parked { spine, slot });
        })
        .unwrap();
    assert!(scratch_in_use(graph, cell) > 0);

    // Step two: both halves come back, the scratch half still reads, and an interior write through
    // the re-anchored `Cell` lands. More scratch is written and the half goes back.
    graph
        .enter(cell, |context| {
            let numbers = context.continuation().expect("the storage half was stored");
            let Parked { spine, slot } = context
                .scratch_continuation()
                .expect("the scratch half was stored");
            assert_eq!((*spine[0], *spine[1], *slot.get()), (11, 12, 10));
            slot.set(spine[1]);
            // A scratch value over a scratch value, and over storage read this step.
            let wider = context.scratch_writer().fill(3, |index| match index {
                0 => slot.get(),
                1 => spine[0],
                _ => &numbers[0],
            });
            context.store_successor(numbers);
            context.store_scratch_successor(Parked { spine: wider, slot });
        })
        .unwrap();
    let held = scratch_in_use(graph, cell);
    assert!(held > 0);

    // Step three: read it out and store nothing back.
    graph
        .enter(cell, |context| {
            let Parked { spine, slot } = context
                .scratch_continuation()
                .expect("the scratch half was stored");
            assert_eq!(
                (*spine[0], *spine[1], *spine[2], *slot.get()),
                (12, 11, 10, 12)
            );
        })
        .unwrap();
    assert_eq!(
        scratch_in_use(graph, cell),
        held,
        "nothing resets at a step's exit"
    );

    // Step four: the slot was empty at entry, so the scratch is gone and storage is not.
    graph
        .enter(cell, |context| {
            let numbers = context.continuation().expect("the storage half was stored");
            assert_eq!(numbers, &[10, 11, 12]);
            assert!(context.scratch_continuation().is_none());
        })
        .unwrap();
    assert_eq!(scratch_in_use(graph, cell), 0);
}

#[test]
fn a_slab_cells_scratch_survives_parks_and_resets_once_unnamed() {
    let mut graph: Graph = CellGraph::new(2, pin);
    let cell = graph.create(None).unwrap();
    round_trip(&mut graph, cell.into());
    graph.release(cell, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(graph.is_empty());
}

#[test]
fn a_tree_cells_scratch_survives_parks_and_resets_once_unnamed() {
    let mut graph: Graph = CellGraph::new(2, pin);
    let root = graph.create(None).unwrap();
    let tree = graph.create_tree(root, None).unwrap();
    round_trip(&mut graph, tree.into());
    graph.release_tree(tree).unwrap();
    graph.release(root, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(graph.is_empty());
}

/// A child's death splices its region into a parent that is holding scratch across the park: the
/// parent's scratch bytes neither move nor drop under the absorb.
#[test]
fn an_absorb_leaves_the_absorbers_named_scratch_alone() {
    let mut graph: Graph = CellGraph::new(2, pin);
    let root = graph.create(None).unwrap();
    let parent = graph.create_tree(root, None).unwrap();
    graph
        .enter(parent, |context| {
            let numbers = context.scratch_writer().fill(1, |_| 41u32);
            let spine = context.scratch_writer().fill(1, |_| &numbers[0]);
            let slot = &context.scratch_writer().fill(1, |_| Cell::new(&numbers[0]))[0];
            context.store_scratch_successor(Parked { spine, slot });
        })
        .unwrap();
    let held = scratch_in_use(&graph, parent);

    // The child pins a value homed in itself up into the parent, which pledges its bump there.
    let child = graph.create_tree(parent, None).unwrap();
    graph
        .enter(child, |context| {
            context.writer().fill(64, |index| index as u64);
            let value = number_here(context, 7);
            context
                .alloc_into::<Number, Number>(
                    parent,
                    &[operand_at(&value, usize::MAX)],
                    |_, views| Active::new(super::pinned(&views[0])),
                )
                .unwrap();
        })
        .unwrap();
    let before = graph.regions.tree_bytes(parent.index());
    graph.release_tree(child).unwrap();
    assert!(
        graph.regions.tree_bytes(parent.index()) > before,
        "the child's bump spliced into the parent's bundle"
    );
    assert_eq!(scratch_in_use(&graph, parent), held);

    graph
        .enter(parent, |context| {
            let Parked { spine, slot } = context
                .scratch_continuation()
                .expect("the scratch half was stored");
            assert_eq!((*spine[0], *slot.get()), (41, 41));
        })
        .unwrap();
    graph.release_tree(parent).unwrap();
    graph.release(root, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(graph.is_empty());
}

#[test]
fn a_departing_cells_scratch_is_dropped_at_disposal() {
    let mut graph: Graph = CellGraph::new(2, pin);
    let producer = graph.create(None).unwrap();
    let consumer = graph.create(None).unwrap();
    graph
        .enter(producer, |context| {
            context.writer().fill(8, |index| index as u32);
            let numbers = context.scratch_writer().fill(512, |index| index as u32);
            let spine = context.scratch_writer().fill(1, |_| &numbers[0]);
            let slot = &context.scratch_writer().fill(1, |_| Cell::new(&numbers[0]))[0];
            context.store_scratch_successor(Parked { spine, slot });
        })
        .unwrap();
    graph
        .enter(consumer, |context| context.hold(producer))
        .unwrap()
        .unwrap();
    let region_bytes = graph.region_bytes(producer).unwrap();
    assert!(scratch_in_use(&graph, producer) > 0);

    // Held and refused, so the producer seals: its region goes into the tier, and its scratch
    // goes nowhere.
    graph.release(producer, ReleaseAbsorption::Refused).unwrap();
    assert_eq!(
        graph
            .regions
            .scratch_in_use(CellHome::Slab(producer.slot())),
        0
    );
    let sealed = graph
        .cells
        .sealed
        .ids()
        .next()
        .expect("the producer sealed");
    assert_eq!(
        graph.cells.sealed_retained_bytes(sealed),
        Some(region_bytes),
        "a seal retains the region and no scratch"
    );
    graph
        .release(consumer, ReleaseAbsorption::IntoHolder)
        .unwrap();
    assert!(graph.is_empty());
}

#[test]
fn scratch_bytes_are_in_no_price() {
    let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let log = std::rc::Rc::clone(&seen);
    let mut graph: Graph = CellGraph::new(2, move |prices: Prices| {
        log.borrow_mut().push(prices);
        Verdict::Pin
    });
    let producer = graph.create(None).unwrap();
    let consumer = graph.create(None).unwrap();
    graph
        .enter(producer, |context| {
            let value = number_here(context, 7);
            context.scratch_writer().fill(4096, |index| index as u64);
            context
                .alloc_into::<Number, Number>(
                    consumer,
                    &[operand_at(&value, usize::MAX)],
                    |_, views| Active::new(super::pinned(&views[0])),
                )
                .unwrap();
        })
        .unwrap();
    let region_bytes = graph.region_bytes(producer).unwrap();
    assert!(scratch_in_use(&graph, producer) > region_bytes);
    assert_eq!(seen.borrow().len(), 1);
    assert_eq!(
        seen.borrow()[0].pin_bytes,
        region_bytes,
        "a pin is billed the region and none of the scratch beside it"
    );
}

#[test]
fn a_store_and_a_read_in_one_step_hand_the_stored_half_back() {
    let mut graph: Graph = CellGraph::new(1, pin);
    let cell = graph.create(None).unwrap();
    graph
        .enter(cell, |context| {
            assert!(context.continuation().is_none());
            assert!(context.scratch_continuation().is_none());
            let numbers = context.writer().fill(2, |index| index as u32);
            let spine = context.scratch_writer().fill(1, |_| &numbers[1]);
            let slot = &context.scratch_writer().fill(1, |_| Cell::new(&numbers[0]))[0];
            context.store_successor(numbers);
            context.store_scratch_successor(Parked { spine, slot });
            assert_eq!(context.continuation(), Some(numbers));
            let parked = context.scratch_continuation().expect("stored a moment ago");
            assert_eq!((*parked.spine[0], *parked.slot.get()), (1, 0));
            // One-shot, both of them.
            assert!(context.continuation().is_none());
            assert!(context.scratch_continuation().is_none());
        })
        .unwrap();
}

#[test]
fn a_panicking_step_leaves_both_halves_as_it_held_them() {
    let mut graph: Graph = CellGraph::new(1, pin);
    let cell = graph.create(None).unwrap();
    graph
        .enter(cell, |context| {
            let numbers = context.writer().fill(2, |index| index as u32);
            context.store_successor(numbers);
        })
        .unwrap();

    // The step takes the storage half, stores a scratch half, and panics holding both as they
    // are: the one it took is gone, and the one it stored is at rest.
    let panicked = catch_unwind(AssertUnwindSafe(|| {
        graph
            .enter(cell, |context| {
                let numbers = context.continuation().expect("the storage half was stored");
                let spine = context.scratch_writer().fill(1, |_| &numbers[1]);
                let slot = &context.scratch_writer().fill(1, |_| Cell::new(&numbers[0]))[0];
                context.store_scratch_successor(Parked { spine, slot });
                panic!("the step fails");
            })
            .unwrap();
    }));
    assert!(panicked.is_err());

    // The executing flag fell with the context, so the cell is enterable again.
    graph
        .enter(cell, |context| {
            assert!(context.continuation().is_none());
            let parked = context
                .scratch_continuation()
                .expect("the scratch half went back to rest under the panic");
            assert_eq!((*parked.spine[0], *parked.slot.get()), (1, 0));
        })
        .unwrap();
}

/// A scratch half over one scratch value, for the shared-region tests: what it names is a byte of
/// the *host's* scratch bump, whichever cell stored it.
fn park_one<'step>(context: &mut StepContext<'static, 'step, '_, '_, Storage, Spine>, value: u32) {
    let numbers = context.scratch_writer().fill(1, |_| value);
    let spine = context.scratch_writer().fill(1, |_| &numbers[0]);
    let slot = &context.scratch_writer().fill(1, |_| Cell::new(&numbers[0]))[0];
    context.store_scratch_successor(Parked { spine, slot });
}

#[test]
fn a_tenants_scratch_is_its_hosts_and_waits_on_every_tenant() {
    let mut graph: Graph = CellGraph::new(1, pin);
    let host = graph.create(None).unwrap();
    let first = graph.create_tenant(host, None).unwrap();
    let second = graph.create_tenant(host, None).unwrap();
    let home = CellHome::Slab(host.slot());

    graph.enter(first, |context| park_one(context, 41)).unwrap();
    let parked = scratch_in_use(&graph, host);
    assert!(parked > 0, "a tenant's scratch is its host's bump");
    assert_eq!(scratch_in_use(&graph, first), parked);
    assert_eq!(graph.cells.tenancy(home).scratch_tenants, 1);

    // The other tenant enters with its own slot empty, and the bump is not handed back: the
    // first tenant's half is at rest over it. It writes beside that half and stores nothing.
    graph
        .enter(second, |context| {
            context.scratch_writer().fill(8, |index| index as u64);
        })
        .unwrap();
    assert!(scratch_in_use(&graph, host) > parked);
    // The host's own step holds the reset off for the same reason.
    graph.enter(host, |_| ()).unwrap();
    assert!(scratch_in_use(&graph, host) > parked);

    // The first tenant's half comes back intact, and it stores nothing this time.
    graph
        .enter(first, |context| {
            let Parked { spine, slot } = context
                .scratch_continuation()
                .expect("the scratch half was stored");
            assert_eq!((*spine[0], *slot.get()), (41, 41));
        })
        .unwrap();
    assert_eq!(graph.cells.tenancy(home).scratch_tenants, 0);
    assert!(
        scratch_in_use(&graph, host) > 0,
        "nothing resets at a step's exit"
    );

    // Nothing names the bump now, so the next `enter` of any of the three hands it back.
    graph.enter(second, |_| ()).unwrap();
    assert_eq!(scratch_in_use(&graph, host), 0);

    graph.release_tenant(first).unwrap();
    graph.release_tenant(second).unwrap();
    graph.release(host, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(graph.is_empty());
}

#[test]
fn a_tenant_dying_with_named_scratch_unblocks_the_reset() {
    let mut graph: Graph = CellGraph::new(1, pin);
    let host = graph.create(None).unwrap();
    let tenant = graph.create_tenant(host, None).unwrap();
    let home = CellHome::Slab(host.slot());
    graph
        .enter(tenant, |context| park_one(context, 41))
        .unwrap();
    assert_eq!(graph.cells.tenancy(home).scratch_tenants, 1);

    graph.enter(host, |_| ()).unwrap();
    assert!(scratch_in_use(&graph, host) > 0);

    graph.release_tenant(tenant).unwrap();
    assert_eq!(graph.cells.tenancy(home), crate::tenant::Tenancy::default());
    graph.enter(host, |_| ()).unwrap();
    assert_eq!(scratch_in_use(&graph, host), 0);
}

#[test]
fn a_released_hosts_scratch_slot_no_longer_blocks_its_tenants() {
    let mut graph: Graph = CellGraph::new(1, pin);
    let host = graph.create(None).unwrap();
    let tenant = graph.create_tenant(host, None).unwrap();
    graph.enter(host, |context| park_one(context, 41)).unwrap();

    // While the host lives, its half holds the reset off for its tenant too.
    graph.enter(tenant, |_| ()).unwrap();
    assert!(scratch_in_use(&graph, tenant) > 0);

    // Its death clears the half — a dead cell is never entered — and the bump stays, for the
    // tenant to have whole at its next step.
    graph.release(host, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(scratch_in_use(&graph, tenant) > 0);
    graph.enter(tenant, |_| ()).unwrap();
    assert_eq!(scratch_in_use(&graph, tenant), 0);

    graph.release_tenant(tenant).unwrap();
    assert!(graph.is_empty());
}
