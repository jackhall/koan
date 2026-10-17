//! A tenant's own lend: what a tenant borrows, named at its creation rather than copied from its
//! host, and the count on its lender that outlives the tenant in its host's scratch
//! ([../../lend.rs](../../lend.rs)).
//!
//! What these pin: a tenant lent a named region lays and reads it at `'lent` while its host's own
//! steps see no lend; a count dropped at the tenant's release would free bytes the host's scratch
//! still names, so it passes to that scratch and drops at its next reset, one per lender; a
//! tenant's lend is copied by its tenants and by `Lend::Of`; a lend equal to the host's own takes
//! no count; a dead host whose lender waits on it resets its scratch and lets the lender go,
//! unless the lender borrows that scratch, which is a lend cycle the audit names; and each refusal
//! changes nothing.
//!
//! The headers and scratch states are the [headers](super::headers) fixture's, so the host's read
//! of a scratch header the tenant laid over its lent region is a read Miri checks.

use super::super::*;
use super::headers::{Graph, new_graph, value_of};
use super::lend::{census, counts, disposed, lay_in_scratch, lay_lent, lent_value};
use super::one;
use crate::lend::Lent;
use crate::tree::TreeState;

/// What the tenant `cell` borrows.
fn tenant_lent(graph: &Graph, cell: TenantHandle) -> Option<Lent> {
    graph.cells.tenants.lent(cell.index())
}

/// A build root with a tree lender under it holding a region header over `value`, and a run root
/// to host tenants: the shape a borrowed specialization takes.
fn lender_and_host(graph: &mut Graph, value: u32) -> (SlabHandle, TreeHandle, SlabHandle) {
    let build = graph.create(None).unwrap();
    let lender = graph.create_tree(build, None).unwrap();
    let laid = graph
        .enter(lender, |context| super::headers::lay(context, value))
        .unwrap();
    assert_eq!(laid, Some(value));
    let run = graph.create(None).unwrap();
    (build, lender, run)
}

#[test]
fn a_tenant_lent_a_named_region_reads_and_lays_it_at_lent() {
    let mut graph = new_graph(2);
    let (build, lender, run) = lender_and_host(&mut graph, 41);
    let tenant = (graph.create_tenant_lent(run, Some(Lend::Named(lender.into())), None)).unwrap();
    assert_eq!(
        tenant_lent(&graph, tenant),
        Some(Lent::Region(CellHome::Tree(lender.index())))
    );
    assert_eq!(counts(&graph, lender).region_borrowers, 1);

    let read = graph
        .enter(tenant, |context| {
            assert!(context.lent_scratch().is_none());
            lay_lent(context, 7)
        })
        .unwrap();
    assert_eq!(read, Some(41), "the lender's header was laid first");
    assert_eq!(
        graph.enter(tenant, |context| lent_value(context)).unwrap(),
        Some(41)
    );
    let host_lent = |context: &mut super::headers::Context<'_, '_, '_, '_>| {
        (
            context.lent_scratch().is_none(),
            context.lent_region().is_none(),
        )
    };
    assert_eq!(graph.enter(run, host_lent).unwrap(), (true, true));

    graph.release_tenant(tenant).unwrap();
    graph.release_tree(lender).unwrap();
    graph.enter(run, |_| ()).unwrap();
    assert!(disposed(&graph, lender));
    graph.release(build, ReleaseAbsorption::IntoHolder).unwrap();
    graph.release(run, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(graph.is_empty());
}

/// The tenant lays a header in its host's scratch over a number in its lent region, and the host
/// reads it after the tenant and the lender are both released. Under Miri a count dropped at the
/// tenant's release would have freed the lender's bytes under that read.
#[test]
fn a_tenants_lend_outlives_its_release_until_its_hosts_scratch_resets() {
    let mut graph = new_graph(2);
    let (build, lender, run) = lender_and_host(&mut graph, 41);
    // The host parks a state over its scratch, so the bump is not reset between steps.
    graph
        .enter(run, |context| {
            context.store_scratch_state(one(context.scratch_writer(), 7));
        })
        .unwrap();
    let tenant = (graph.create_tenant_lent(run, Some(Lend::Named(lender.into())), None)).unwrap();
    let laid = graph
        .enter(tenant, |context| {
            let (writer, _) = context.lent_region().expect("the tenant is lent a region");
            lay_in_scratch(context, one(writer, 5))
        })
        .unwrap();
    assert_eq!(laid, Some(5));

    graph.release_tree(lender).unwrap();
    let read = graph
        .enter(tenant, |context| {
            context.scratch_header().get().and_then(value_of)
        })
        .unwrap();
    assert_eq!(read, Some(5));

    graph.release_tenant(tenant).unwrap();
    assert!(!disposed(&graph, lender), "the host's scratch holds it");
    assert_eq!(counts(&graph, lender).region_borrowers, 1);
    let read = graph
        .enter(run, |context| {
            let parked = context.scratch_state().copied();
            (parked, context.scratch_header().get().and_then(value_of))
        })
        .unwrap();
    assert_eq!(read, (Some(7), Some(5)));
    // That step took the parked state and left none, so its end reset the scratch.
    assert!(disposed(&graph, lender));

    graph.release(build, ReleaseAbsorption::IntoHolder).unwrap();
    graph.release(run, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(graph.is_empty());
}

#[test]
fn one_hosts_scratch_holds_one_count_per_lender() {
    let mut graph = new_graph(2);
    let (build, lender, run) = lender_and_host(&mut graph, 1);
    let lend = Some(Lend::Named(lender.into()));
    let first = graph.create_tenant_lent(run, lend, None).unwrap();
    let second = graph.create_tenant_lent(run, lend, None).unwrap();
    assert_eq!(counts(&graph, lender).region_borrowers, 2);

    graph.release_tenant(first).unwrap();
    assert_eq!(counts(&graph, lender).region_borrowers, 2);
    graph.release_tenant(second).unwrap();
    assert_eq!(counts(&graph, lender).region_borrowers, 1);
    assert_eq!(graph.cells.held.values().flatten().count(), 1);

    graph.enter(run, |_| ()).unwrap();
    assert_eq!(counts(&graph, lender).region_borrowers, 0);
    assert!(graph.cells.held.is_empty());

    graph.release_tree(lender).unwrap();
    graph.release(build, ReleaseAbsorption::IntoHolder).unwrap();
    graph.release(run, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(graph.is_empty());
}

#[test]
fn a_tenant_copies_a_lend_of_its_own_and_its_tenants_copy_it() {
    let mut graph = new_graph(2);
    let (build, lender, run) = lender_and_host(&mut graph, 41);
    let region = Lent::Region(CellHome::Tree(lender.index()));
    let lent = Some(region);
    let borrowing =
        (graph.create_tenant_lent(run, Some(Lend::Named(lender.into())), None)).unwrap();
    let nested = graph.create_tenant(borrowing, None).unwrap();
    let copied = (graph.create_tenant_lent(run, Some(Lend::Of(borrowing.into())), None)).unwrap();
    for tenant in [nested, copied] {
        assert_eq!(tenant_lent(&graph, tenant), lent);
        assert_eq!(
            graph.enter(tenant, |context| lent_value(context)).unwrap(),
            Some(41)
        );
    }
    let tree = (graph.create_tree_lent(run, Lend::Of(borrowing.into()), None)).unwrap();
    assert_eq!(graph.cells.trees.lent(tree.index()), region);
    assert_eq!(counts(&graph, lender).region_borrowers, 4);

    // A tenant lent nothing gives nothing, though its host is the same.
    let bare = graph.create_tenant_lent(run, None, None).unwrap();
    let under_bare = graph.create_tenant(bare, None).unwrap();
    assert_eq!(tenant_lent(&graph, bare), None);
    assert_eq!(tenant_lent(&graph, under_bare), None);
    assert_eq!(
        graph.create_tree_lent(run, Lend::Of(bare.into()), None),
        Err(CreateLentError::Unlendable(bare.into()))
    );

    for tenant in [borrowing, nested, copied, bare, under_bare] {
        graph.release_tenant(tenant).unwrap();
    }
    graph.release_tree(tree).unwrap();
    graph.enter(run, |_| ()).unwrap();
    assert_eq!(counts(&graph, lender).region_borrowers, 0);
    graph.release_tree(lender).unwrap();
    graph.release(build, ReleaseAbsorption::IntoHolder).unwrap();
    graph.release(run, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(graph.is_empty());
}

#[test]
fn a_lend_equal_to_the_hosts_takes_no_count() {
    let mut graph = new_graph(2);
    let (build, lender, run) = lender_and_host(&mut graph, 1);
    let host = (graph.create_tree_lent(run, Lend::Named(lender.into()), None)).unwrap();
    let under_scratch = graph.create_tree(run, None).unwrap();
    assert_eq!(counts(&graph, lender).region_borrowers, 1);
    assert_eq!(counts(&graph, run).scratch_borrowers, 1);

    let named = (graph.create_tenant_lent(host, Some(Lend::Named(lender.into())), None)).unwrap();
    let copied = (graph.create_tenant_lent(host, Some(Lend::Of(host.into())), None)).unwrap();
    let scratch =
        (graph.create_tenant_lent(under_scratch, Some(Lend::Of(under_scratch.into())), None))
            .unwrap();
    assert_eq!(
        tenant_lent(&graph, scratch),
        Some(Lent::Scratch(CellHome::Slab(run.slot())))
    );
    assert_eq!(counts(&graph, lender).region_borrowers, 1);
    assert_eq!(counts(&graph, run).scratch_borrowers, 1);

    for tenant in [named, copied, scratch] {
        graph.release_tenant(tenant).unwrap();
    }
    assert!(graph.cells.held.is_empty());
    graph.release_tree(host).unwrap();
    graph.release_tree(under_scratch).unwrap();
    graph.release_tree(lender).unwrap();
    graph.release(build, ReleaseAbsorption::IntoHolder).unwrap();
    graph.release(run, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(graph.is_empty());
}

/// A dead host's scratch resets in the disposal walk once nothing names it, so a lender that waits
/// on the host goes. A lender that borrows the host's scratch is a cycle the audit names: neither
/// it nor the host ever disposes.
#[test]
fn a_dead_hosts_lender_disposes_unless_it_borrows_the_hosts_scratch() {
    let mut graph = new_graph(1);
    let root = graph.create(None).unwrap();

    // The lender borrows the host's region, so the host waits on it.
    let host = graph.create_tree(root, None).unwrap();
    let lender = (graph.create_tree_lent(root, Lend::Named(host.into()), None)).unwrap();
    let tenant = (graph.create_tenant_lent(host, Some(Lend::Named(lender.into())), None)).unwrap();
    assert_eq!(graph.lend_cycle(), None);
    graph.release_tenant(tenant).unwrap();
    graph.release_tree(host).unwrap();
    assert!(
        graph.cells.held.is_empty(),
        "the dead host's scratch reset in the walk"
    );
    assert!(!disposed(&graph, host), "the host waits on its borrower");
    graph.release_tree(lender).unwrap();
    assert!(disposed(&graph, lender));
    assert!(disposed(&graph, host));

    // The lender is the host's child, borrowing its scratch.
    let host = graph.create_tree(root, None).unwrap();
    let lender = graph.create_tree(host, None).unwrap();
    let tenant = (graph.create_tenant_lent(host, Some(Lend::Named(lender.into())), None)).unwrap();
    let cycle = Some(vec![CellHandle::from(lender), host.into()]);
    assert_eq!(graph.lend_cycle(), cycle);
    graph.release_tenant(tenant).unwrap();
    graph.release_tree(lender).unwrap();
    graph.release_tree(host).unwrap();
    graph.release(root, ReleaseAbsorption::IntoHolder).unwrap();
    assert_eq!(graph.lend_cycle(), cycle);
    assert_eq!(graph.cells.trees.state(lender.index()), TreeState::Dead);
    assert_eq!(graph.cells.trees.state(host.index()), TreeState::Dead);
    assert!(!graph.is_empty());
}

#[test]
fn a_tenant_lend_refuses_what_it_cannot_borrow_and_changes_nothing() {
    let mut graph = new_graph(3);
    let (build, lender, run) = lender_and_host(&mut graph, 1);
    let host_tenant = graph.create_tenant(run, None).unwrap();
    let borrower = (graph.create_tree_lent(build, Lend::Named(run.into()), None)).unwrap();
    let other = graph.create_tree(build, None).unwrap();
    let stale = graph.create_tree(build, None).unwrap();
    graph.release_tree(stale).unwrap();
    let gone = graph.create(None).unwrap();
    graph.release(gone, ReleaseAbsorption::IntoHolder).unwrap();

    let refusals: [(CellHandle, Lend, CreateLentError); 9] = [
        (
            gone.into(),
            Lend::Named(lender.into()),
            CreateLentError::Under(Stale(gone.into())),
        ),
        (
            run.into(),
            Lend::ParentScratch,
            CreateLentError::Unlendable(run.into()),
        ),
        (
            run.into(),
            Lend::Named(stale.into()),
            CreateLentError::Lender(Stale(stale.into())),
        ),
        (
            run.into(),
            Lend::Of(stale.into()),
            CreateLentError::Lender(Stale(stale.into())),
        ),
        (
            run.into(),
            Lend::Named(host_tenant.into()),
            CreateLentError::Unlendable(host_tenant.into()),
        ),
        (
            run.into(),
            Lend::Of(run.into()),
            CreateLentError::Unlendable(run.into()),
        ),
        (
            run.into(),
            Lend::Of(other.into()),
            CreateLentError::ForeignScratch(other.into()),
        ),
        // The host's own region, named, through a tenant of it, and copied.
        (
            run.into(),
            Lend::Named(run.into()),
            CreateLentError::Unlendable(run.into()),
        ),
        (
            host_tenant.into(),
            Lend::Of(borrower.into()),
            CreateLentError::Unlendable(host_tenant.into()),
        ),
    ];
    let before = census(&graph);
    for (host, lend, refusal) in refusals {
        assert_eq!(
            graph.create_tenant_lent(host, Some(lend), None),
            Err(refusal)
        );
        assert_eq!(census(&graph), before);
    }

    graph.release_tenant(host_tenant).unwrap();
    for cell in [borrower, other, lender] {
        graph.release_tree(cell).unwrap();
    }
    graph.release(build, ReleaseAbsorption::IntoHolder).unwrap();
    graph.release(run, ReleaseAbsorption::IntoHolder).unwrap();
    assert!(graph.is_empty());
}
