//! A tree cell's lend: what it borrows from its creation to its disposal, and the count on its
//! lender that keeps the borrowed half in place ([../../tree/README.md § Death](../../tree/README.md#death)).
//!
//! What these pin: a lender's count drops at its borrower's disposal, not its release, so a dead
//! borrower waiting on a child keeps its lender's scratch from resetting while its own scratch
//! still names the lent bytes; `create_tree` lends the parent's scratch; a named lend hands a step
//! the lender's region at `'lent`, and a tenant's step its host's lend; a lender's disposal waits
//! on its region borrowers, off the borrower's chain or on it, and happens once; a copied lend
//! takes a count of its own, and a copied scratch lend is taken only under its lender; a named
//! lend mints no pin; and each refusal changes nothing. Beside them, the drain's reads between
//! steps: `peek` reads a live cell's region header and refuses any other name, and `is_childless`
//! counts dead children that have not disposed.
//!
//! The headers and scratch states are the [headers](super::headers) fixture's, so a read through a
//! lent door is a read of the chunk the header names, which Miri checks.

use std::cell::Cell;

use super::super::*;
use super::headers::{Context, Graph, Mark, lay, new_graph, table, value_of};
use super::one;
use crate::lend::Lent;
use crate::tenant::Tenancy;
use crate::tree::TreeState;

/// Lay a table over `number` in the step's own scratch header. `number` lives in the lent bump,
/// which the cell's scratch may name since `'lent: 'scratch`; the coercion to `'scratch` comes
/// before the `Cell`, which is invariant.
pub(super) fn lay_in_scratch<'scratch>(
    context: &Context<'_, '_, 'scratch, '_>,
    number: &'scratch u32,
) -> Option<u32> {
    value_of(
        context
            .scratch_header()
            .get_or_lay(|| Mark::Table(one(context.scratch_writer(), Cell::new(number)))),
    )
}

/// What the tree cell `cell` borrows.
fn lent_of(graph: &Graph, cell: TreeHandle) -> Lent {
    graph.cells.trees.lent(cell.index())
}

/// The counts a slab or tree cell carries, live or dead.
pub(super) fn counts(graph: &Graph, cell: impl Into<CellHandle>) -> Tenancy {
    graph.cells.tenancy(match cell.into() {
        CellHandle::Slab(cell) => CellHome::Slab(cell.slot()),
        CellHandle::Tree(cell) => CellHome::Tree(cell.index()),
        CellHandle::Tenant(_) => panic!("a tenant carries no counts"),
    })
}

/// Every occupied tree slot with its state, and every count on every undisposed write home: what
/// a refusal must leave as it found it.
pub(super) fn census(graph: &Graph) -> (Vec<(u32, TreeState)>, Vec<Tenancy>) {
    let trees: Vec<(u32, TreeState)> = (graph.cells.trees.occupied())
        .map(|index| (index, graph.cells.trees.state(index)))
        .collect();
    let slabs = (0..graph.cells.cap).map(|slot| graph.cells.tenancy(CellHome::Slab(slot)));
    let tree_counts = (trees.iter())
        .filter(|(_, state)| matches!(state, TreeState::Live | TreeState::Dead))
        .map(|(index, _)| graph.cells.tenancy(CellHome::Tree(*index)));
    let counts = slabs.chain(tree_counts).collect();
    (trees, counts)
}

/// Lay a table over `value` in the step's lent region's header, or read back the one there.
pub(super) fn lay_lent(context: &Context<'_, '_, '_, '_>, value: u32) -> Option<u32> {
    let (writer, header) = context.lent_region().expect("the cell is lent a region");
    value_of(header.get_or_lay(|| table(writer, value)))
}

/// What the step's lent region's header points at.
pub(super) fn lent_value(context: &Context<'_, '_, '_, '_>) -> Option<u32> {
    let (_, header) = context.lent_region().expect("the cell is lent a region");
    header.get().and_then(value_of)
}

/// Whether the tree cell `cell` names has left the pool: recycled, or a tombstone.
pub(super) fn disposed(graph: &Graph, cell: TreeHandle) -> bool {
    !matches!(
        graph.cells.trees.state(cell.index()),
        TreeState::Live | TreeState::Dead
    ) || graph.cells.trees.occupant(cell.index()) != cell
}

/// The scratch bytes `cell`'s bump holds.
fn scratch_in_use(graph: &Graph, cell: SlabHandle) -> usize {
    graph.regions.scratch_in_use(CellHome::Slab(cell.slot()))
}

#[test]
fn a_lenders_count_drops_at_its_borrowers_disposal_not_its_release() {
    let mut graph = new_graph(1);
    let root = graph.create(None).unwrap();
    let parent = graph.create_tree(root, None).unwrap();
    let child = graph.create_tree(parent, None).unwrap();
    let home = CellHome::Slab(root.slot());
    assert_eq!(graph.cells.tenancy(home).scratch_borrowers, 1);

    graph.release_tree(parent).unwrap();
    assert!(!graph.is_live(parent));
    assert_eq!(
        graph.cells.trees.state(parent.index()),
        TreeState::Dead,
        "the parent waits on its child"
    );
    assert_eq!(
        graph.cells.tenancy(home).scratch_borrowers,
        1,
        "a dead borrower still counts"
    );

    graph.release_tree(child).unwrap();
    assert_ne!(graph.cells.trees.state(parent.index()), TreeState::Dead);
    assert_eq!(graph.cells.tenancy(home).scratch_borrowers, 0);

    graph.release(root, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(graph.is_empty());
}

/// A dead borrower's scratch header names its lender's scratch, and its child reads it through
/// its own lent doors after the lender steps. Under Miri a reset rebuilds the bump, so a count
/// dropped at the borrower's release would make the child's read a use after free.
#[test]
fn a_dead_borrowers_scratch_holds_its_lenders_scratch_until_its_disposal() {
    let mut graph = new_graph(1);
    let root = graph.create(None).unwrap();
    let parent = graph.create_tree(root, None).unwrap();
    let child = graph.create_tree(parent, None).unwrap();

    let laid = graph
        .enter(parent, |context| {
            let (writer, _) = context.lent_scratch().expect("a tree cell has a lender");
            lay_in_scratch(context, one(writer, 41))
        })
        .unwrap();
    assert_eq!(laid, Some(41));

    graph.release_tree(parent).unwrap();
    graph.enter(root, |_| ()).unwrap();
    assert!(
        scratch_in_use(&graph, root) > 0,
        "the dead parent still holds the root's reset off"
    );
    let read = graph
        .enter(child, |context| {
            let (_, header) = context.lent_scratch().expect("a tree cell has a lender");
            header.get().and_then(value_of)
        })
        .unwrap();
    assert_eq!(read, Some(41));

    graph.release_tree(child).unwrap();
    graph.enter(root, |_| ()).unwrap();
    assert_eq!(scratch_in_use(&graph, root), 0);

    graph.release(root, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(graph.is_empty());
}

#[test]
fn create_tree_lends_its_parents_scratch() {
    let mut graph = new_graph(1);
    let root = graph.create(None).unwrap();
    let plain = graph.create_tree(root, None).unwrap();
    let named = (graph.create_tree_lent(root, Lend::ParentScratch, None)).unwrap();
    let under_tree = graph.create_tree(plain, None).unwrap();
    let tenant = graph.create_tenant(plain, None).unwrap();
    let under_tenant = graph.create_tree(tenant, None).unwrap();
    let named_under_tenant = (graph.create_tree_lent(tenant, Lend::ParentScratch, None)).unwrap();

    let at_root = Lent::Scratch(CellHome::Slab(root.slot()));
    let at_plain = Lent::Scratch(CellHome::Tree(plain.index()));
    assert_eq!(lent_of(&graph, plain), at_root);
    assert_eq!(lent_of(&graph, named), at_root);
    assert_eq!(lent_of(&graph, under_tree), at_plain);
    assert_eq!(lent_of(&graph, under_tenant), at_plain);
    assert_eq!(lent_of(&graph, named_under_tenant), at_plain);
    assert_eq!(counts(&graph, root).scratch_borrowers, 2);
    assert_eq!(counts(&graph, plain).scratch_borrowers, 3);

    let lent_halves = |context: &mut Context<'_, '_, '_, '_>| {
        (
            context.lent_scratch().is_some(),
            context.lent_region().is_some(),
        )
    };
    for cell in [plain, named, under_tree, under_tenant, named_under_tenant] {
        assert_eq!(graph.enter(cell, lent_halves).unwrap(), (true, false));
    }
    assert_eq!(graph.enter(tenant, lent_halves).unwrap(), (true, false));
    assert_eq!(graph.enter(root, lent_halves).unwrap(), (false, false));

    for cell in [plain, named, under_tree, under_tenant, named_under_tenant] {
        graph.release_tree(cell).unwrap();
    }
    graph.release_tenant(tenant).unwrap();
    graph.release(root, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(graph.is_empty());
}

/// A run cell lent a build cell's region reads the header the lender laid and lays in that region
/// at `'lent`, parks what it laid in its own scratch, and reads both back after the region table
/// moved and the lender was released.
#[test]
fn a_named_lend_reads_and_lays_in_its_lenders_region_at_lent() {
    let mut graph = new_graph(2);
    let build = graph.create(None).unwrap();
    let run = graph.create(None).unwrap();
    let lender = graph.create_tree(build, None).unwrap();
    assert_eq!(
        graph.enter(lender, |context| lay(context, 41)).unwrap(),
        Some(41)
    );
    let borrower = (graph.create_tree_lent(run, Lend::Named(lender.into()), None)).unwrap();
    assert_eq!(
        lent_of(&graph, borrower),
        Lent::Region(CellHome::Tree(lender.index()))
    );

    let read = graph
        .enter(borrower, |context| {
            assert!(context.lent_scratch().is_none());
            let (writer, _) = context.lent_region().expect("the cell is lent a region");
            context.store_scratch_state(one(writer, 7));
            lent_value(context)
        })
        .unwrap();
    assert_eq!(read, Some(41));

    // Enough tree cells that the tree half of the region table reallocates.
    let others: Vec<TreeHandle> = (0..8)
        .map(|_| graph.create_tree(run, None).unwrap())
        .collect();
    graph.release_tree(lender).unwrap();
    assert!(!graph.is_live(lender));
    assert!(!disposed(&graph, lender), "the borrower holds the lender");

    let read = graph
        .enter(borrower, |context| {
            let parked = context.scratch_state().copied();
            (parked, lent_value(context))
        })
        .unwrap();
    assert_eq!(read, (Some(7), Some(41)));

    for cell in others {
        graph.release_tree(cell).unwrap();
    }
    graph.release_tree(borrower).unwrap();
    assert!(disposed(&graph, lender));
    graph.release(build, ReleaseAbsorption::IntoHolder).unwrap();
    graph.release(run, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(graph.is_empty());
}

#[test]
fn a_named_lend_of_a_slab_cell_holds_its_slot_past_its_death() {
    let mut graph = new_graph(2);
    let lender = graph.create(None).unwrap();
    let root = graph.create(None).unwrap();
    let borrower = (graph.create_tree_lent(root, Lend::Named(lender.into()), None)).unwrap();
    assert_eq!(counts(&graph, lender).region_borrowers, 1);

    graph
        .release(lender, ReleaseAbsorption::IntoHolder)
        .unwrap();
    assert!(!graph.is_live(lender));
    assert_eq!(
        graph.cells.slots[lender.slot() as usize].state,
        SlabState::Dead
    );
    let laid = graph
        .enter(borrower, |context| lay_lent(context, 5))
        .unwrap();
    assert_eq!(laid, Some(5));

    graph.release_tree(borrower).unwrap();
    assert_eq!(
        graph.cells.slots[lender.slot() as usize].state,
        SlabState::Free
    );
    graph.release(root, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(graph.is_empty());
}

#[test]
fn a_lender_off_the_chain_disposes_with_its_last_borrower() {
    let mut graph = new_graph(2);
    let build = graph.create(None).unwrap();
    let outer = graph.create_tree(build, None).unwrap();
    let inner = graph.create_tree(outer, None).unwrap();
    let run = graph.create(None).unwrap();
    let borrower = (graph.create_tree_lent(run, Lend::Named(inner.into()), None)).unwrap();

    graph.release_tree(outer).unwrap();
    graph.release_tree(inner).unwrap();
    assert!(!disposed(&graph, inner), "the borrower holds the lender");
    assert!(!disposed(&graph, outer), "the lender holds its parent");

    graph.release_tree(borrower).unwrap();
    assert!(disposed(&graph, borrower));
    assert!(disposed(&graph, inner));
    assert!(disposed(&graph, outer));
    assert_eq!(graph.cells.tree_children_of(build), 0);

    graph.release(build, ReleaseAbsorption::IntoHolder).unwrap();
    graph.release(run, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(graph.is_empty());
}

#[test]
fn a_lender_on_the_chain_disposes_once() {
    let mut graph = new_graph(2);
    // A tree parent lending its own region to its child, released first.
    let root = graph.create(None).unwrap();
    let parent = graph.create_tree(root, None).unwrap();
    let child = (graph.create_tree_lent(parent, Lend::Named(parent.into()), None)).unwrap();
    graph.release_tree(parent).unwrap();
    assert_eq!(counts(&graph, parent).region_borrowers, 1);
    graph.release_tree(child).unwrap();
    assert!(disposed(&graph, parent));
    assert_eq!(graph.cells.tree_children_of(root), 0);
    assert_eq!(counts(&graph, root), Tenancy::default());

    // A root lending its own region to its child, released first.
    let other = graph.create(None).unwrap();
    let under = (graph.create_tree_lent(other, Lend::Named(other.into()), None)).unwrap();
    graph.release(other, ReleaseAbsorption::IntoHolder).unwrap();
    assert_eq!(
        graph.cells.slots[other.slot() as usize].state,
        SlabState::Dead
    );
    graph.release_tree(under).unwrap();
    assert_eq!(
        graph.cells.slots[other.slot() as usize].state,
        SlabState::Free
    );

    graph.release(root, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(graph.is_empty());
}

#[test]
fn a_copied_lend_takes_a_count_of_its_own() {
    let mut graph = new_graph(2);
    let build = graph.create(None).unwrap();
    let lender = graph.create_tree(build, None).unwrap();
    let run = graph.create(None).unwrap();
    let first = (graph.create_tree_lent(run, Lend::Named(lender.into()), None)).unwrap();
    assert_eq!(
        graph.enter(first, |context| lay_lent(context, 41)).unwrap(),
        Some(41)
    );
    let second = (graph.create_tree_lent(run, Lend::Of(first.into()), None)).unwrap();
    assert_eq!(lent_of(&graph, second), lent_of(&graph, first));
    assert_eq!(counts(&graph, lender).region_borrowers, 2);

    graph.release_tree(first).unwrap();
    assert_eq!(counts(&graph, lender).region_borrowers, 1);
    assert_eq!(
        graph.enter(second, |context| lent_value(context)).unwrap(),
        Some(41)
    );

    graph.release_tree(lender).unwrap();
    assert!(!disposed(&graph, lender));
    graph.release_tree(second).unwrap();
    assert!(disposed(&graph, lender));

    graph.release(build, ReleaseAbsorption::IntoHolder).unwrap();
    graph.release(run, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(graph.is_empty());
}

#[test]
fn a_lend_copied_from_a_cell_whose_lender_is_dead_is_taken() {
    let mut graph = new_graph(2);
    let lender = graph.create(None).unwrap();
    let run = graph.create(None).unwrap();
    let first = (graph.create_tree_lent(run, Lend::Named(lender.into()), None)).unwrap();
    graph
        .release(lender, ReleaseAbsorption::IntoHolder)
        .unwrap();

    let second = (graph.create_tree_lent(run, Lend::Of(first.into()), None)).unwrap();
    assert_eq!(
        lent_of(&graph, second),
        Lent::Region(CellHome::Slab(lender.slot()))
    );
    assert_eq!(counts(&graph, lender).region_borrowers, 2);

    graph.release_tree(first).unwrap();
    graph.release_tree(second).unwrap();
    graph.release(run, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(graph.is_empty());
}

#[test]
fn a_copied_scratch_lend_is_taken_only_under_its_lender() {
    let mut graph = new_graph(2);
    let parent = graph.create(None).unwrap();
    let copied = graph.create_tree(parent, None).unwrap();
    let tenant = graph.create_tenant(parent, None).unwrap();
    let at_parent = Lent::Scratch(CellHome::Slab(parent.slot()));

    let sibling = (graph.create_tree_lent(parent, Lend::Of(copied.into()), None)).unwrap();
    let under_tenant = (graph.create_tree_lent(tenant, Lend::Of(copied.into()), None)).unwrap();
    assert_eq!(lent_of(&graph, sibling), at_parent);
    assert_eq!(lent_of(&graph, under_tenant), at_parent);
    assert_eq!(counts(&graph, parent).scratch_borrowers, 3);

    let other = graph.create(None).unwrap();
    let before = census(&graph);
    for under in [CellHandle::from(other), copied.into()] {
        assert_eq!(
            graph.create_tree_lent(under, Lend::Of(copied.into()), None),
            Err(CreateLentError::ForeignScratch(copied.into()))
        );
        assert_eq!(census(&graph), before);
    }

    for cell in [copied, sibling, under_tenant] {
        graph.release_tree(cell).unwrap();
    }
    graph.release_tenant(tenant).unwrap();
    graph
        .release(parent, ReleaseAbsorption::IntoHolder)
        .unwrap();
    graph.release(other, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(graph.is_empty());
}

#[test]
fn a_tenant_of_a_named_borrower_is_handed_its_hosts_lend() {
    let mut graph = new_graph(2);
    let lender = graph.create(None).unwrap();
    let run = graph.create(None).unwrap();
    let borrower = (graph.create_tree_lent(run, Lend::Named(lender.into()), None)).unwrap();
    let tenant = graph.create_tenant(borrower, None).unwrap();

    let laid = graph
        .enter(tenant, |context| {
            assert!(context.lent_scratch().is_none());
            lay_lent(context, 9)
        })
        .unwrap();
    assert_eq!(laid, Some(9));
    let read = graph
        .enter(lender, |context| context.header().get().and_then(value_of))
        .unwrap();
    assert_eq!(read, Some(9));

    // Copying a tenant's lend copies its host's.
    let copied = (graph.create_tree_lent(run, Lend::Of(tenant.into()), None)).unwrap();
    assert_eq!(
        lent_of(&graph, copied),
        Lent::Region(CellHome::Slab(lender.slot()))
    );
    assert_eq!(counts(&graph, lender).region_borrowers, 2);
    assert_eq!(
        graph.enter(copied, |context| lent_value(context)).unwrap(),
        Some(9)
    );
    graph.release_tree(copied).unwrap();

    graph.release_tenant(tenant).unwrap();
    graph.release_tree(borrower).unwrap();
    graph
        .release(lender, ReleaseAbsorption::IntoHolder)
        .unwrap();
    graph.release(run, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(graph.is_empty());
}

#[test]
fn a_named_lend_mints_no_pin() {
    let mut graph = new_graph(2);
    let lender = graph.create(None).unwrap();
    let run = graph.create(None).unwrap();
    let borrower = (graph.create_tree_lent(run, Lend::Named(lender.into()), None)).unwrap();
    assert_eq!(
        graph
            .enter(borrower, |context| lay_lent(context, 3))
            .unwrap(),
        Some(3)
    );

    assert!(
        !graph
            .cells
            .pins
            .held_by_any(0..graph.cells.cap, lender.slot())
    );
    assert!(!graph.cells.pins.test(run.slot(), lender.slot()));

    graph.release_tree(borrower).unwrap();
    graph
        .release(lender, ReleaseAbsorption::IntoHolder)
        .unwrap();
    graph.release(run, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(graph.is_empty());
}

#[test]
fn a_lend_refuses_a_stale_dead_or_unlendable_name() {
    let mut graph = new_graph(2);
    let root = graph.create(None).unwrap();
    let dead_slab = graph.create(None).unwrap();
    let under_dead_slab = graph.create_tree(dead_slab, None).unwrap();
    graph
        .release(dead_slab, ReleaseAbsorption::IntoHolder)
        .unwrap();
    let slab_tenant = graph.create_tenant(root, None).unwrap();
    let dead = graph.create_tree(root, None).unwrap();
    let holding = graph.create_tree(dead, None).unwrap();
    graph.release_tree(dead).unwrap();
    let stale = graph.create_tree(root, None).unwrap();
    graph.release_tree(stale).unwrap();
    let live = graph.create_tree(root, None).unwrap();
    let tree_tenant = graph.create_tenant(live, None).unwrap();

    let refusals: [(CellHandle, Lend, CreateLentError); 11] = [
        (
            dead_slab.into(),
            Lend::ParentScratch,
            CreateLentError::Under(Stale(dead_slab.into())),
        ),
        (
            root.into(),
            Lend::Named(dead_slab.into()),
            CreateLentError::Lender(Stale(dead_slab.into())),
        ),
        (
            stale.into(),
            Lend::ParentScratch,
            CreateLentError::Under(Stale(stale.into())),
        ),
        (
            dead.into(),
            Lend::ParentScratch,
            CreateLentError::Under(Stale(dead.into())),
        ),
        (
            root.into(),
            Lend::Named(dead.into()),
            CreateLentError::Lender(Stale(dead.into())),
        ),
        (
            root.into(),
            Lend::Of(dead.into()),
            CreateLentError::Lender(Stale(dead.into())),
        ),
        (
            root.into(),
            Lend::Named(stale.into()),
            CreateLentError::Lender(Stale(stale.into())),
        ),
        (
            root.into(),
            Lend::Of(stale.into()),
            CreateLentError::Lender(Stale(stale.into())),
        ),
        (
            root.into(),
            Lend::Named(tree_tenant.into()),
            CreateLentError::Unlendable(tree_tenant.into()),
        ),
        (
            live.into(),
            Lend::Of(root.into()),
            CreateLentError::Unlendable(root.into()),
        ),
        (
            live.into(),
            Lend::Of(slab_tenant.into()),
            CreateLentError::Unlendable(slab_tenant.into()),
        ),
    ];
    let before = census(&graph);
    for (parent, lend, refusal) in refusals {
        assert_eq!(graph.create_tree_lent(parent, lend, None), Err(refusal));
        assert_eq!(census(&graph), before);
    }

    graph.release_tenant(tree_tenant).unwrap();
    graph.release_tenant(slab_tenant).unwrap();
    graph.release_tree(live).unwrap();
    graph.release_tree(holding).unwrap();
    graph.release_tree(under_dead_slab).unwrap();
    graph.release(root, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(graph.is_empty());
}

/// The number a peeked header points at.
fn peeked(graph: &Graph, cell: impl Into<CellHandle>) -> Result<Option<Option<u32>>, PeekError> {
    graph.peek(cell, |header, _| value_of(header.into_value()))
}

/// A header laid in a tree cell's region is read between steps, before and after the region grows
/// under it.
#[test]
fn peek_reads_a_header_laid_in_a_live_region() {
    let mut graph = new_graph(1);
    let root = graph.create(None).unwrap();
    let cell = graph.create_tree(root, None).unwrap();
    assert_eq!(
        graph.enter(cell, |context| lay(context, 41)).unwrap(),
        Some(41)
    );
    assert_eq!(peeked(&graph, cell), Ok(Some(Some(41))));

    graph
        .enter(cell, |context| {
            context.writer().fill(64, |index| index as u64);
        })
        .unwrap();
    assert_eq!(peeked(&graph, cell), Ok(Some(Some(41))));

    graph.release_tree(cell).unwrap();
    graph.release(root, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(graph.is_empty());
}

#[test]
fn peek_refuses_a_dead_stale_or_tenant_handle() {
    let mut graph = new_graph(2);
    let root = graph.create(None).unwrap();
    assert_eq!(peeked(&graph, root), Ok(None), "nothing is laid");
    graph.enter(root, |context| lay(context, 2)).unwrap();
    assert_eq!(peeked(&graph, root), Ok(Some(Some(2))));

    let dead = graph.create_tree(root, None).unwrap();
    let child = graph.create_tree(dead, None).unwrap();
    graph.release_tree(dead).unwrap();
    assert_eq!(
        peeked(&graph, dead),
        Err(PeekError::Stale(Stale(dead.into())))
    );
    let tenant = graph.create_tenant(root, None).unwrap();
    assert_eq!(peeked(&graph, tenant), Err(PeekError::Tenant(tenant)));
    let gone = graph.create(None).unwrap();
    graph.release(gone, ReleaseAbsorption::IntoHolder).unwrap();
    assert_eq!(
        peeked(&graph, gone),
        Err(PeekError::Stale(Stale(gone.into())))
    );

    graph.release_tenant(tenant).unwrap();
    graph.release_tree(child).unwrap();
    graph.release(root, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(graph.is_empty());
}

#[test]
fn is_childless_counts_undisposed_children() {
    let mut graph = new_graph(1);
    let root = graph.create(None).unwrap();
    let parent = graph.create_tree(root, None).unwrap();
    assert_eq!(graph.is_childless(parent), Ok(true));

    let child = graph.create_tree(parent, None).unwrap();
    assert_eq!(graph.is_childless(parent), Ok(false));
    graph.release_tree(child).unwrap();
    assert_eq!(graph.is_childless(parent), Ok(true));

    let waiting = graph.create_tree(parent, None).unwrap();
    let grandchild = graph.create_tree(waiting, None).unwrap();
    graph.release_tree(waiting).unwrap();
    assert_eq!(
        graph.is_childless(parent),
        Ok(false),
        "a released child waits on its own"
    );
    assert_eq!(graph.is_childless(waiting), Err(Stale(waiting)));
    graph.release_tree(grandchild).unwrap();
    assert_eq!(graph.is_childless(parent), Ok(true));
    assert_eq!(graph.is_childless(grandchild), Err(Stale(grandchild)));

    graph.release_tree(parent).unwrap();
    graph.release(root, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(graph.is_empty());
}
