//! Tenancy: a cell with no region of its own, whose every step writes its **host**'s
//! ([../../tenant.rs](../../tenant.rs)).
//!
//! What these pin, for a slab host and for a tree host: a tenant step writes, lifts, places and
//! redeems as its host, and embeds a host-homed borrow through its own writer at no price; a
//! host's `'here` borrow survives a tenant appending under it; a tenant's death is a count
//! decrement; a released host waits, region in place, on its last tenant; and a live tenant named
//! as a host, a tree parent or a placement destination means its host, whatever has been declared
//! about that host since.

use std::cell::RefCell;
use std::rc::Rc;

use super::super::*;
use super::{Number, Owned, number_here, one, operand_at, pin, pinned, state_of};
use crate::tree::Ancestor;

/// A continuation over a run in the cell's region.
struct Numbers;
crate::reattachable!(Numbers => &'cell [u32]);

/// A continuation over a spine of borrows, each of which has to be at `'here` to be stored.
struct Spine;
crate::reattachable!(Spine => &'cell [&'cell u32]);

/// A host of one of the two kinds a region lives in, under a root that is the host itself for a
/// slab host.
struct Hosted<C: Reattachable<'static>> {
    graph: CellGraph<'static, C>,
    root: SlabHandle,
    host: CellHandle,
    home: CellHome,
}

impl<C: Reattachable<'static>> Hosted<C> {
    fn slab(cap: u32, verdict: impl FnMut(Prices) -> Verdict + 'static) -> Self {
        let mut graph = CellGraph::new(cap, verdict);
        let root = graph.create(None).unwrap();
        Hosted {
            graph,
            root,
            host: root.into(),
            home: CellHome::Slab(root.slot()),
        }
    }

    fn tree(cap: u32, verdict: impl FnMut(Prices) -> Verdict + 'static) -> Self {
        let mut graph = CellGraph::new(cap, verdict);
        let root = graph.create(None).unwrap();
        let tree = graph.create_tree(root, None).unwrap();
        Hosted {
            graph,
            root,
            host: tree.into(),
            home: CellHome::Tree(tree.index()),
        }
    }

    /// Chunk bytes of the host's region bundle, live or dead-but-undisposed.
    fn host_bytes(&self) -> usize {
        self.graph.regions.region(self.home).allocated_bytes()
    }

    /// The name a value homed in the host is kept under.
    fn host_key(&self) -> HomeHandle {
        match self.host {
            CellHandle::Slab(handle) => HomeHandle::Slab(handle),
            CellHandle::Tree(handle) => HomeHandle::Tree(handle),
            CellHandle::Tenant(_) => unreachable!("a fixture's host owns a region"),
        }
    }

    /// Declare the host's death, by whichever door its kind has.
    fn release_host(&mut self) {
        match self.host {
            CellHandle::Slab(handle) => self
                .graph
                .release(handle, ReleaseAbsorption::IntoHolder)
                .unwrap(),
            CellHandle::Tree(handle) => self.graph.release_tree(handle).unwrap(),
            CellHandle::Tenant(_) => unreachable!("a fixture's host owns a region"),
        }
    }

    /// Release whatever of the fixture is still live, and check nothing is left.
    fn wind_down(mut self) {
        if self.graph.is_live(self.host) {
            self.release_host();
        }
        if self.graph.is_live(self.root) {
            self.graph
                .release(self.root, ReleaseAbsorption::IntoHolder)
                .unwrap();
        }
        assert!(self.graph.is_empty());
    }
}

fn a_tenant_writes_into_its_hosts_region(mut hosted: Hosted<Owned>) {
    let tenant = hosted.graph.create_tenant(hosted.host, None).unwrap();
    assert_eq!(hosted.host_bytes(), 0, "a tenant draws no bump");

    let (cell, dormant) = hosted
        .graph
        .enter(tenant, |context| {
            let value = number_here(context, 41);
            (context.cell(), context.keep(value))
        })
        .unwrap();
    assert_eq!(
        cell,
        CellHandle::Tenant(tenant),
        "a step runs in the tenant"
    );
    assert!(hosted.host_bytes() > 0, "and writes the host's region");
    assert_eq!(
        dormant.key().home,
        hosted.host_key(),
        "where its value is homed"
    );

    // The host reads it back as its own.
    let read = hosted
        .graph
        .enter(hosted.host, |context| {
            let carrier = context.redeem(dormant).unwrap();
            *context.read(&carrier).value()
        })
        .unwrap();
    assert_eq!(read, 41);

    hosted.graph.release_tenant(tenant).unwrap();
    hosted.wind_down();
}

#[test]
fn a_tenant_writes_into_its_slab_hosts_region() {
    a_tenant_writes_into_its_hosts_region(Hosted::slab(1, pin));
}

#[test]
fn a_tenant_writes_into_its_tree_hosts_region() {
    a_tenant_writes_into_its_hosts_region(Hosted::tree(1, pin));
}

#[test]
fn a_tenants_carrier_reaches_its_host() {
    let mut hosted: Hosted<Owned> = Hosted::slab(2, pin);
    let consumer = hosted.graph.create(None).unwrap();
    let tenant = hosted.graph.create_tenant(hosted.host, None).unwrap();
    hosted
        .graph
        .enter(tenant, |context| {
            let value = number_here(context, 7);
            context
                .alloc_into::<Number, Number>(
                    consumer,
                    &[operand_at(&value, usize::MAX)],
                    |_, views| Active::new(pinned(&views[0])),
                )
                .unwrap();
        })
        .unwrap();
    assert!(
        hosted
            .graph
            .cells
            .pins
            .test(consumer.slot(), hosted.root.slot()),
        "the consumer's row names the host, whose region the pinned value lives in"
    );
}

/// The acceptance shape: a value a tenant writes embeds a host-homed borrow with no operand, no
/// pin and no price. The borrow is acquired once through the doors that exist — a redeem and an
/// own-cell crossing, which the verdict is shown at zero and which mints nothing — and from then
/// on it is an ordinary `'here` reference that a plain write embeds and a successor keeps.
#[test]
fn a_tenant_embeds_a_host_homed_borrow_through_its_own_writer() {
    let seen = Rc::new(RefCell::new(Vec::new()));
    let log = Rc::clone(&seen);
    let mut hosted: Hosted<Spine> = Hosted::slab(1, move |prices: Prices| {
        log.borrow_mut().push(prices);
        Verdict::Pin
    });
    let dormant = hosted
        .graph
        .enter(hosted.host, |context| {
            let value = number_here(context, 41);
            context.keep(value)
        })
        .unwrap();
    let tenant = hosted.graph.create_tenant(hosted.host, None).unwrap();
    let slot = hosted.root.slot();
    let row_before = *hosted.graph.cells.pins.row(slot);
    let holders_before = hosted.graph.cells.pins.holders(slot);

    hosted
        .graph
        .enter(tenant, |context| {
            let carrier = context.redeem(dormant).unwrap();
            let borrowed: &u32 = context
                .alloc_here(&[operand_at(&carrier, usize::MAX)], |_, views| {
                    pinned(&views[0])
                });
            // The embed itself: the tenant's own writer, no operand, no build.
            let spine = context.writer().fill(1, |_| borrowed);
            context.store_successor(spine);
        })
        .unwrap();
    assert_eq!(
        seen.borrow().len(),
        1,
        "one crossing was put to the verdict"
    );
    assert_eq!(
        seen.borrow()[0].pin_bytes,
        0,
        "and it was free: the host is home"
    );
    assert!(*hosted.graph.cells.pins.row(slot) == row_before, "no pin");
    assert_eq!(hosted.graph.cells.pins.holders(slot), holders_before);

    // A later tenant step reads through the spine it kept.
    let read = hosted
        .graph
        .enter(tenant, |context| {
            let spine = context.continuation().expect("the spine was stored");
            *spine[0]
        })
        .unwrap();
    assert_eq!(read, 41);
    assert_eq!(
        seen.borrow().len(),
        1,
        "the embed and the store priced nothing"
    );

    hosted.graph.release_tenant(tenant).unwrap();
    hosted.wind_down();
}

fn a_hosts_borrow_survives_a_tenant_appending_under_it(mut hosted: Hosted<Numbers>) {
    let tenant = hosted.graph.create_tenant(hosted.host, None).unwrap();
    hosted
        .graph
        .enter(hosted.host, |context| {
            let numbers = context.writer().fill(4, |index| index as u32 + 10);
            context.store_successor(numbers);
        })
        .unwrap();
    let before = hosted.host_bytes();

    // The host is parked over `numbers` while the tenant appends enough to claim a new chunk.
    hosted
        .graph
        .enter(tenant, |context| {
            let appended = context.writer().fill(1024, |index| index as u64);
            assert_eq!(appended[1023], 1023);
        })
        .unwrap();
    assert!(
        hosted.host_bytes() > before,
        "the tenant's append grew the host's bump past its first chunk"
    );

    hosted
        .graph
        .enter(hosted.host, |context| {
            let numbers = context.continuation().expect("the host parked over it");
            assert_eq!(numbers, &[10, 11, 12, 13]);
        })
        .unwrap();
    hosted.graph.release_tenant(tenant).unwrap();
    hosted.wind_down();
}

#[test]
fn a_slab_hosts_borrow_survives_a_tenant_appending_under_it() {
    a_hosts_borrow_survives_a_tenant_appending_under_it(Hosted::slab(1, pin));
}

#[test]
fn a_tree_hosts_borrow_survives_a_tenant_appending_under_it() {
    a_hosts_borrow_survives_a_tenant_appending_under_it(Hosted::tree(1, pin));
}

fn a_tenants_death_is_a_count_decrement(mut hosted: Hosted<Owned>) {
    let tenant = hosted.graph.create_tenant(hosted.host, None).unwrap();
    assert_eq!(hosted.graph.cells.tenancy(hosted.home).tenants, 1);
    let dormant = hosted
        .graph
        .enter(tenant, |context| {
            let value = number_here(context, 41);
            context.keep(value)
        })
        .unwrap();
    let bytes = hosted.host_bytes();
    let pool = hosted.graph.cells.trees().occupied().count();
    let relocations = hosted.graph.cells.relocations();

    hosted.graph.release_tenant(tenant).unwrap();
    assert!(!hosted.graph.is_live(tenant));
    assert_eq!(hosted.graph.cells.tenancy(hosted.home).tenants, 0);
    assert_eq!(hosted.host_bytes(), bytes, "no reclaim and no splice");
    assert_eq!(
        hosted.graph.cells.trees().occupied().count(),
        pool,
        "no tombstone"
    );
    assert_eq!(hosted.graph.cells.relocations(), relocations);
    assert!(hosted.graph.is_live(hosted.host));

    // What the tenant kept was the host's all along.
    let read = hosted
        .graph
        .enter(hosted.host, |context| {
            let carrier = context.redeem(dormant).unwrap();
            *context.read(&carrier).value()
        })
        .unwrap();
    assert_eq!(read, 41);
    hosted.wind_down();
}

#[test]
fn a_slab_tenants_death_is_a_count_decrement() {
    a_tenants_death_is_a_count_decrement(Hosted::slab(1, pin));
}

#[test]
fn a_tree_tenants_death_is_a_count_decrement() {
    a_tenants_death_is_a_count_decrement(Hosted::tree(1, pin));
}

#[test]
fn a_released_slab_host_waits_on_its_tenants() {
    let mut hosted: Hosted<Owned> = Hosted::slab(1, pin);
    let tenant = hosted.graph.create_tenant(hosted.host, None).unwrap();
    hosted.release_host();
    assert!(!hosted.graph.is_live(hosted.host));
    assert_eq!(state_of(&hosted.graph, hosted.root), SlabState::Dead);
    assert_eq!(
        hosted.graph.create(None),
        Err(CreateError::SlabFull),
        "the slot is not reused while a tenant writes the region in it"
    );

    // The tenant runs on, in the dead host's region.
    assert!(hosted.graph.is_live(tenant));
    hosted
        .graph
        .enter(tenant, |context| {
            one(context.writer(), 7u32);
        })
        .unwrap();
    assert!(hosted.host_bytes() > 0);

    hosted.graph.release_tenant(tenant).unwrap();
    assert!(
        hosted.graph.is_empty(),
        "the last tenant's release disposes the host"
    );
}

#[test]
fn a_released_tree_host_waits_on_its_tenants_and_the_cascade_reaches_its_root() {
    let mut hosted: Hosted<Owned> = Hosted::tree(1, pin);
    let tenant = hosted.graph.create_tenant(hosted.host, None).unwrap();
    hosted.release_host();
    hosted
        .graph
        .release(hosted.root, ReleaseAbsorption::IntoHolder)
        .unwrap();
    assert!(!hosted.graph.is_live(hosted.host));
    assert_eq!(state_of(&hosted.graph, hosted.root), SlabState::Dead);

    hosted
        .graph
        .enter(tenant, |context| {
            one(context.writer(), 7u32);
        })
        .unwrap();
    assert!(hosted.host_bytes() > 0);

    // The tenant was the last thing the tree host waited on, and the tree host the last thing
    // its released root did.
    hosted.graph.release_tenant(tenant).unwrap();
    assert!(hosted.graph.is_empty());
}

/// The sharing tail loop: the first host is released while its tenants run, and every hop has
/// only a live tenant to name.
#[test]
fn a_tenant_named_as_a_host_resolves_to_its_host() {
    let mut hosted: Hosted<Owned> = Hosted::slab(1, pin);
    let mut current = hosted.graph.create_tenant(hosted.host, None).unwrap();
    hosted.release_host();

    for hop in 0..8u32 {
        let next = hosted.graph.create_tenant(current, None).unwrap();
        assert_eq!(
            hosted.graph.cells.tenants.host(next.index()),
            hosted.home,
            "a tenant is never itself a host"
        );
        let before = hosted.host_bytes();
        hosted
            .graph
            .enter(next, |context| {
                context.writer().fill(64, |index| index as u32 + hop);
            })
            .unwrap();
        assert!(hosted.host_bytes() >= before);
        hosted.graph.release_tenant(current).unwrap();
        assert_eq!(hosted.graph.cells.tenancy(hosted.home).tenants, 1);
        current = next;
    }
    assert_eq!(state_of(&hosted.graph, hosted.root), SlabState::Dead);
    hosted.graph.release_tenant(current).unwrap();
    assert!(hosted.graph.is_empty());
}

fn a_tree_child_created_under_a_tenant_is_its_hosts_child(mut hosted: Hosted<Owned>) {
    let tenant = hosted.graph.create_tenant(hosted.host, None).unwrap();
    let child = hosted.graph.create_tree(tenant, None).unwrap();
    let expected = match hosted.home {
        CellHome::Slab(_) => Ancestor::Root,
        CellHome::Tree(index) => Ancestor::Tree(index),
    };
    let pool = hosted.graph.cells.trees();
    assert_eq!(pool.parent(child.index()), expected);
    assert_eq!(pool.root(child.index()), hosted.root.slot());

    // An upward pin into the tenant is an upward pin into the host: the child pledges there.
    hosted
        .graph
        .enter(child, |context| {
            let value = number_here(context, 7);
            context
                .alloc_into::<Number, Number>(
                    tenant,
                    &[operand_at(&value, usize::MAX)],
                    |_, views| Active::new(pinned(&views[0])),
                )
                .unwrap();
        })
        .unwrap();
    assert_eq!(
        hosted.graph.cells.trees().pledge(child.index()),
        Some(expected)
    );

    let before = hosted.host_bytes();
    hosted.graph.release_tree(child).unwrap();
    assert!(
        hosted.host_bytes() > before,
        "the child spliced into the host"
    );
    hosted.graph.release_tenant(tenant).unwrap();
    hosted.wind_down();
}

#[test]
fn a_tree_child_created_under_a_slab_hosts_tenant_is_the_hosts_child() {
    a_tree_child_created_under_a_tenant_is_its_hosts_child(Hosted::slab(1, pin));
}

#[test]
fn a_tree_child_created_under_a_tree_hosts_tenant_is_the_hosts_child() {
    a_tree_child_created_under_a_tenant_is_its_hosts_child(Hosted::tree(1, pin));
}

#[test]
fn a_placement_into_a_tenant_lands_in_its_host() {
    let mut hosted: Hosted<Owned> = Hosted::slab(2, pin);
    let producer = hosted.graph.create(None).unwrap();
    let tenant = hosted.graph.create_tenant(hosted.host, None).unwrap();
    let dormant = hosted
        .graph
        .enter(producer, |context| {
            let placed = context
                .alloc_into::<Number, Number>(tenant, &[], |writer, _| {
                    Active::new(one(writer, 41u32))
                })
                .unwrap();
            context.keep(placed)
        })
        .unwrap();
    assert_eq!(dormant.key().home, hosted.host_key());
    assert!(hosted.host_bytes() > 0);

    // A released tenant is a stale destination, reported under the name that was given.
    hosted.graph.release_tenant(tenant).unwrap();
    let refused = hosted
        .graph
        .enter(producer, |context| {
            context
                .alloc_into::<Number, Number>(tenant, &[], |writer, _| {
                    Active::new(one(writer, 0u32))
                })
                .map(|_| ())
        })
        .unwrap();
    assert_eq!(refused.unwrap_err().name(), CellHandle::Tenant(tenant));
}

#[test]
fn a_tenant_redeems_what_its_host_may() {
    let mut hosted: Hosted<Owned> = Hosted::slab(3, pin);
    let producer = hosted.graph.create(None).unwrap();
    let stranger = hosted.graph.create(None).unwrap();
    let tenant = hosted.graph.create_tenant(hosted.host, None).unwrap();
    let outsider = hosted.graph.create_tenant(stranger, None).unwrap();

    // One value homed in the host, one homed in a producer the host holds.
    let host = hosted.host;
    let (in_host, in_producer) = hosted
        .graph
        .enter(producer, |context| {
            let placed = context
                .alloc_into::<Number, Number>(host, &[], |writer, _| {
                    Active::new(one(writer, 41u32))
                })
                .unwrap();
            let own = number_here(context, 42);
            (context.keep(placed), context.keep(own))
        })
        .unwrap();
    hosted
        .graph
        .enter(hosted.host, |context| context.hold(producer))
        .unwrap()
        .unwrap();

    let read = hosted
        .graph
        .enter(tenant, |context| {
            let first = context.redeem(in_host).unwrap();
            let second = context.redeem(in_producer).unwrap();
            (
                *context.read(&first).value(),
                *context.read(&second).value(),
            )
        })
        .unwrap();
    assert_eq!(read, (41, 42));

    let refused = hosted
        .graph
        .enter(outsider, |context| {
            (
                context.redeem(in_host).map(|_| ()),
                context.redeem(in_producer).map(|_| ()),
            )
        })
        .unwrap();
    assert_eq!(
        refused,
        (Err(RedeemError::Unheld), Err(RedeemError::Unheld)),
        "entitlement is the host's, and the stranger holds neither"
    );
}

#[test]
fn the_tenant_doors_refuse_a_name_kept_past_a_declared_death() {
    let mut hosted: Hosted<Owned> = Hosted::slab(1, pin);
    let tenant = hosted.graph.create_tenant(hosted.host, None).unwrap();
    hosted.graph.release_tenant(tenant).unwrap();

    assert_eq!(
        hosted.graph.enter(tenant, |_| ()),
        Err(EnterError::Stale(Stale(CellHandle::Tenant(tenant))))
    );
    assert_eq!(
        hosted.graph.release_tenant(tenant),
        Err(ReleaseTenantError::Stale(Stale(tenant)))
    );
    assert_eq!(
        hosted.graph.create_tenant(tenant, None),
        Err(Stale(CellHandle::Tenant(tenant)))
    );

    // A dead host named directly is stale too: only a live tenant reaches a dead host.
    let root = hosted.root;
    hosted.release_host();
    assert_eq!(
        hosted.graph.create_tenant(root, None),
        Err(Stale(CellHandle::Slab(root)))
    );
    assert!(hosted.graph.is_empty());
}

#[test]
fn a_tenant_is_entered_by_one_step_at_a_time_and_reuses_its_index() {
    let mut hosted: Hosted<Owned> = Hosted::slab(1, pin);
    let first = hosted.graph.create_tenant(hosted.host, None).unwrap();
    hosted.graph.release_tenant(first).unwrap();
    let second = hosted
        .graph
        .create_tenant(hosted.host, Some(String::from("next")))
        .unwrap();
    assert_eq!(second.index(), first.index());
    assert_ne!(second.generation(), first.generation());
    assert!(!hosted.graph.is_live(first));

    let continuation = hosted
        .graph
        .enter(second, |context| context.continuation())
        .unwrap();
    assert_eq!(continuation.as_deref(), Some("next"));
    hosted.graph.release_tenant(second).unwrap();
    hosted.wind_down();
}
