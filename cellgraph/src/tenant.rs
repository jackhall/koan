//! The tenant pool: cells that own no region and write their **host**'s. See
//! [../README.md](../README.md) § The cell.
//!
//! A tenant is the composition the other two kinds cannot express — many units of work, one region.
//! It has what a step needs and nothing a region needs: the two continuation halves, an executing
//! flag and a generation, plus the write home of its host, a slab or tree cell named at its
//! creation. A step in it is handed the host's writer and the host's scratch writer, places and
//! mints as the host, and redeems what the host may; so a value it writes embeds a host-homed
//! borrow like any own-region write, with no operand, no pin and no price.
//!
//! What a host carries for its tenants is two counts ([`Tenancy`]). The first holds its disposal
//! off: a host whose death is declared waits, region in place, until its last tenant leaves —
//! which is what makes the bare slot or pool index a tenant keeps a sound name for it. The second
//! holds its scratch bump's reset off while any tenant names it — with a scratch half at rest, or
//! with a receipt run, which is the tenant's own though the bytes under it are the host's.
//!
//! A tenant has no dead-but-undisposed state. Nothing can be under one — a tenant named as a
//! parent, a host or a destination means its host — so its release takes it out at once, and its
//! death is a count decrement: no reclaim, no splice, no pledge, no tombstone.
//!
//! No cap, like the tree pool: a tenant is in no relation, so nothing about the slab's width
//! bounds it.

use crate::carrier::CellHome;
use crate::handle::{Stale, TenantHandle};
use crate::reattach::{Erased, Reattachable};
use crate::receipt::Delivery;
use crate::slots::StepSlots;

/// What a region-owning cell carries for the tenants that write its region.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) struct Tenancy {
    /// Tenants created on this cell and not yet released. The second thing, beside an undisposed
    /// tree child, that keeps a cell in its table past its death.
    pub(crate) tenants: u32,
    /// How many of those name this cell's scratch bump — a scratch half at rest, or a receipt run.
    /// They share this cell's scratch bump, so its reset waits on this count as well as on the
    /// cell's own slots. Moved where a tenant step ends, where a run is laid down and where a
    /// tenant leaves, and read only where a step ends.
    pub(crate) scratch_tenants: u32,
}

/// One tenant.
struct Tenant<'graph, C: Reattachable<'graph>, S: Reattachable<'graph>, D: Delivery<'graph>> {
    /// The cell whose region this tenant writes, by bare slot or pool index: the host cannot
    /// dispose while this tenant is counted on it, so the index names the same occupant for the
    /// tenant's whole life.
    host: CellHome,
    executing: bool,
    /// What the tenant carries between its steps. The receipt run among them is the tenant's own;
    /// only the bytes it lives in — the host's scratch bump — are the host's.
    step_slots: StepSlots<'graph, C, S, D>,
}

/// One pool index: its generation, and its occupant if it has one.
struct TenantCell<'graph, C: Reattachable<'graph>, S: Reattachable<'graph>, D: Delivery<'graph>> {
    generation: u32,
    occupant: Option<Tenant<'graph, C, S, D>>,
}

/// What a released tenant leaves its host to settle: which host, and whether the tenant's scratch
/// half was at rest and so in the host's count.
pub(crate) struct Departed {
    pub(crate) host: CellHome,
    pub(crate) scratch_named: bool,
}

/// The growable pool of tenants.
pub(crate) struct TenantPool<
    'graph,
    C: Reattachable<'graph>,
    S: Reattachable<'graph>,
    D: Delivery<'graph>,
> {
    slots: Vec<TenantCell<'graph, C, S, D>>,
    free: Vec<u32>,
}

impl<'graph, C: Reattachable<'graph>, S: Reattachable<'graph>, D: Delivery<'graph>>
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

    /// Take a slot for a new tenant of `host`.
    pub(crate) fn create(
        &mut self,
        host: CellHome,
        continuation: Option<Erased<'graph, C>>,
    ) -> TenantHandle {
        let occupant = Some(Tenant {
            host,
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

    /// Take the tenant out: its halves drop, its index frees under a fresh generation, and what
    /// its host has to settle comes back.
    pub(crate) fn remove(&mut self, index: u32) -> Departed {
        let cell = &mut self.slots[index as usize];
        let tenant = cell
            .occupant
            .take()
            .unwrap_or_else(|| panic!("tenant pool slot {index} is free"));
        cell.generation = cell.generation.wrapping_add(1);
        self.free.push(index);
        Departed {
            host: tenant.host,
            scratch_named: tenant.step_slots.names_scratch(),
        }
    }

    /// Every live tenant's host and whether it names that host's scratch bump — what the property
    /// test recounts the hosts' tallies from.
    #[cfg(test)]
    pub(crate) fn census(&self) -> impl Iterator<Item = (CellHome, bool)> + '_ {
        self.slots.iter().filter_map(|cell| {
            cell.occupant
                .as_ref()
                .map(|tenant| (tenant.host, tenant.step_slots.names_scratch()))
        })
    }
}
