//! The tenant pool: cells that own no region and write their **host**'s. See
//! [../README.md](../README.md) § The cell.
//!
//! A tenant is the composition the other two kinds cannot express — many units of work, one region.
//! It has what a step needs and nothing a region needs: the two parked slots, an executing
//! flag and a generation, plus the write home of its host, a slab or tree cell named at its
//! creation, and its lend ([lend.rs](lend.rs)), the half it is handed at `'lent`, resolved at its
//! creation too. A step in it is handed the host's writer and the host's scratch writer, places and
//! mints as the host, and redeems what the host may; so a value it writes embeds a host-homed
//! borrow like any own-region write, with no operand, no pin and no price.
//!
//! What a host carries for its tenants is two counts ([`Tenancy`]). The first holds its disposal
//! off: a host whose death is declared waits, region in place, until its last tenant leaves —
//! which is what makes the bare slot or pool index a tenant keeps a sound name for it. The second
//! holds its scratch bump's reset off while any tenant names it — with a scratch state at rest, or
//! with a receipt run, which is the tenant's own though the bytes under it are the host's. The
//! same struct carries the counts of the tree cells that borrow a half of the cell's storage
//! ([lend.rs](lend.rs)), since each holds that half in place the way a tenant does.
//!
//! A tenant has no dead-but-undisposed state. Nothing can be under one — a tenant named as a
//! parent, a host or a destination means its host — so its release takes it out at once, and its
//! death is a count decrement: no reclaim, no splice, no pledge, no tombstone. A lend it counted
//! on its lender outlives it in its host's scratch, which may still name the lent bytes, until
//! that scratch next resets ([lend.rs](lend.rs)).
//!
//! No cap, like the tree pool: a tenant is in no relation, so nothing about the slab's width
//! bounds it.

use crate::carrier::CellHome;
use crate::handle::{Stale, TenantHandle};
use crate::lend::Lent;
use crate::reattach::{Erased, Reattachable, ReattachableOverBoth};
use crate::receipt::{Delivery, ReceiptRun};
use crate::slots::StepSlots;

/// What a region-owning cell carries for the tenants that write its region and the tree cells
/// that borrow from it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) struct Tenancy {
    /// Tenants created on this cell and not yet released. The second thing, beside an undisposed
    /// tree child, that keeps a cell in its table past its death.
    pub(crate) tenants: u32,
    /// How many of those name this cell's scratch bump — a scratch state at rest, or a receipt
    /// run.
    /// They share this cell's scratch bump, so its reset waits on this count as well as on the
    /// cell's own slots. Moved where a tenant step ends, where a run is laid down and where a
    /// tenant leaves, and read only where a step ends.
    pub(crate) scratch_tenants: u32,
    /// Tree cells borrowing this cell's scratch bump, created and not yet disposed: each holds the
    /// bump's reset off, since a dead borrower's scratch may still name the lent bytes
    /// ([`lent_scratch`](crate::StepContext::lent_scratch)). Each is this cell's own child, so
    /// the child count already holds this cell's disposal.
    pub(crate) scratch_borrowers: u32,
    /// Borrowers of this cell's region, each holding this cell's disposal off, since the
    /// borrower's doors are minted from the region table at this cell's index
    /// ([`lent_region`](crate::StepContext::lent_region)): a tree cell created and not yet
    /// disposed, a tenant whose lend is not its host's own, and a host's scratch holding the count
    /// of such a tenant since its release ([lend.rs](crate::lend)).
    pub(crate) region_borrowers: u32,
}

impl Tenancy {
    /// Whether these counts hold the cell's disposal off: a tenant writing its region, or a tree
    /// cell borrowing it. A scratch borrower is left out: it is a child, and the child count
    /// covers it.
    pub(crate) fn holds_disposal(&self) -> bool {
        self.tenants > 0 || self.region_borrowers > 0
    }
}

/// One tenant.
struct Tenant<'graph, C: Reattachable<'graph>, S: ReattachableOverBoth<'graph>, D: Delivery<'graph>>
{
    /// The cell whose region this tenant writes, by bare slot or pool index: the host cannot
    /// dispose while this tenant is counted on it, so the index names the same occupant for the
    /// tenant's whole life.
    host: CellHome,
    /// What a step in the tenant is handed at `'lent`, resolved at its creation and never changed.
    lent: Option<Lent>,
    executing: bool,
    /// What the tenant carries between its steps. The receipt run among them is the tenant's own;
    /// only the bytes it lives in — the host's scratch bump — are the host's.
    step_slots: StepSlots<'graph, C, S, D>,
}

/// One pool index: its generation, and its occupant if it has one.
struct TenantCell<
    'graph,
    C: Reattachable<'graph>,
    S: ReattachableOverBoth<'graph>,
    D: Delivery<'graph>,
> {
    generation: u32,
    occupant: Option<Tenant<'graph, C, S, D>>,
}

/// What a released tenant leaves its host to settle: which host, what the tenant borrowed,
/// whether its scratch state was at rest and so in the host's count, and the receipt run it left
/// in the host's bump.
pub(crate) struct Departed<'graph, D: Delivery<'graph>> {
    pub(crate) host: CellHome,
    pub(crate) lent: Option<Lent>,
    pub(crate) scratch_named: bool,
    pub(crate) run: Option<Erased<'graph, ReceiptRun<D>>>,
}

/// The growable pool of tenants.
pub(crate) struct TenantPool<
    'graph,
    C: Reattachable<'graph>,
    S: ReattachableOverBoth<'graph>,
    D: Delivery<'graph>,
> {
    slots: Vec<TenantCell<'graph, C, S, D>>,
    free: Vec<u32>,
}

impl<'graph, C: Reattachable<'graph>, S: ReattachableOverBoth<'graph>, D: Delivery<'graph>>
    TenantPool<'graph, C, S, D>
{
    pub(crate) fn new(cap: u32) -> Self {
        TenantPool {
            slots: Vec::with_capacity(cap as usize),
            free: Vec::new(),
        }
    }

    /// Whether the pool holds no tenant. Part of the graph's end-of-program alarm.
    pub(crate) fn is_empty(&self) -> bool {
        self.free.len() == self.slots.len()
    }

    /// Take a slot for a new tenant of `host`, borrowing `lent`.
    pub(crate) fn create(
        &mut self,
        host: CellHome,
        lent: Option<Lent>,
        continuation: Option<Erased<'graph, C>>,
    ) -> TenantHandle {
        let occupant = Some(Tenant {
            host,
            lent,
            executing: false,
            step_slots: StepSlots::born(continuation),
        });
        match self.free.pop() {
            Some(index) => {
                let cell = &mut self.slots[index as usize];
                cell.occupant = occupant;
                TenantHandle::new(index, cell.generation)
            }
            None => {
                self.slots.push(TenantCell {
                    generation: 0,
                    occupant,
                });
                TenantHandle::new((self.slots.len() - 1) as u32, 0)
            }
        }
    }

    /// The pool index a handle names, if that index still holds the tenant it was minted for.
    pub(crate) fn live_index(&self, handle: TenantHandle) -> Result<u32, Stale<TenantHandle>> {
        match self.slots.get(handle.index() as usize) {
            Some(cell) if cell.generation == handle.generation() && cell.occupant.is_some() => {
                Ok(handle.index())
            }
            _ => Err(Stale(handle)),
        }
    }

    fn tenant(&self, index: u32) -> &Tenant<'graph, C, S, D> {
        self.slots[index as usize]
            .occupant
            .as_ref()
            .unwrap_or_else(|| panic!("tenant pool slot {index} is free"))
    }

    fn tenant_mut(&mut self, index: u32) -> &mut Tenant<'graph, C, S, D> {
        self.slots[index as usize]
            .occupant
            .as_mut()
            .unwrap_or_else(|| panic!("tenant pool slot {index} is free"))
    }

    /// The cell whose region the tenant writes.
    pub(crate) fn host(&self, index: u32) -> CellHome {
        self.tenant(index).host
    }

    /// What the tenant borrows.
    pub(crate) fn lent(&self, index: u32) -> Option<Lent> {
        self.tenant(index).lent
    }

    pub(crate) fn is_executing(&self, index: u32) -> bool {
        self.tenant(index).executing
    }

    pub(crate) fn set_executing(&mut self, index: u32, executing: bool) {
        self.tenant_mut(index).executing = executing;
    }

    /// The slots the tenant carries between its steps, over its host's scratch bump.
    pub(crate) fn step_slots(&self, index: u32) -> &StepSlots<'graph, C, S, D> {
        &self.tenant(index).step_slots
    }

    pub(crate) fn step_slots_mut(&mut self, index: u32) -> &mut StepSlots<'graph, C, S, D> {
        &mut self.tenant_mut(index).step_slots
    }

    /// Take the tenant out: both its slots drop, its index frees under a fresh generation, and what
    /// its host has to settle comes back.
    pub(crate) fn remove(&mut self, index: u32) -> Departed<'graph, D> {
        let cell = &mut self.slots[index as usize];
        let mut tenant = cell
            .occupant
            .take()
            .unwrap_or_else(|| panic!("tenant pool slot {index} is free"));
        cell.generation = cell.generation.wrapping_add(1);
        self.free.push(index);
        let scratch_named = tenant.step_slots.names_scratch();
        Departed {
            host: tenant.host,
            lent: tenant.lent,
            scratch_named,
            run: tenant.step_slots.clear_scratch(),
        }
    }

    /// Every live tenant's host, whether it names that host's scratch bump, and what it borrows —
    /// what the lend-cycle audit reads, and the property test recounts the hosts' and lenders'
    /// tallies from.
    pub(crate) fn census(&self) -> impl Iterator<Item = (CellHome, bool, Option<Lent>)> + '_ {
        self.slots.iter().filter_map(|cell| {
            cell.occupant
                .as_ref()
                .map(|tenant| (tenant.host, tenant.step_slots.names_scratch(), tenant.lent))
        })
    }
}
