//! Cell identity, over all three kinds: a slab slot, a tree-pool index or a tenant-pool index, paired
//! with the generation of the occupant that place held when the handle was minted. [`CellHandle`]
//! is the three of them as one name, which is what a door that takes a destination, a parent or a
//! host asks for. See
//! [../README.md](../README.md) § The cell and
//! [tree/README.md](tree/README.md).

/// A name for one cell: the slab slot it occupies, plus the generation that distinguishes it from
/// every other occupant of that slot. `Copy`, so a handle is passed around freely; naming a cell
/// grants nothing, because every verb re-checks the generation against the slot.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct SlabHandle {
    slot: u32,
    generation: u32,
}

impl SlabHandle {
    pub(crate) fn new(slot: u32, generation: u32) -> Self {
        SlabHandle { slot, generation }
    }

    /// The slab slot this handle names.
    pub fn slot(self) -> u32 {
        self.slot
    }

    /// The generation of the occupant this handle was minted for.
    pub fn generation(self) -> u32 {
        self.generation
    }
}

/// A name for one tree cell: the pool index it occupies, plus the generation that distinguishes it
/// from every other occupant of that index. `Copy`, like [`SlabHandle`], and re-checked by every
/// verb for the same reason.
///
/// The pool is separate from the slab and takes no cap, so a tree handle names no slab bit and no
/// mask ever holds one. See [tree/README.md](tree/README.md).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct TreeHandle {
    index: u32,
    generation: u32,
}

impl TreeHandle {
    pub(crate) fn new(index: u32, generation: u32) -> Self {
        TreeHandle { index, generation }
    }

    /// The pool index this handle names.
    pub fn index(self) -> u32 {
        self.index
    }

    /// The generation of the occupant this handle was minted for.
    pub fn generation(self) -> u32 {
        self.generation
    }
}

/// A name for one tenant: the tenant-pool index it occupies, plus the generation that distinguishes
/// it from every other occupant of that index. `Copy`, like [`SlabHandle`], and re-checked by every
/// verb for the same reason.
///
/// A tenant owns no region: a step in it writes its **host**'s, a slab or tree cell named when the
/// tenant was created. So a tenant handle names a place a step runs in and no place storage lives
/// in — nothing is ever homed in a tenant, and a door that asks for a destination, a parent or a
/// host and is given a live tenant means the tenant's host.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct TenantHandle {
    index: u32,
    generation: u32,
}

impl TenantHandle {
    pub(crate) fn new(index: u32, generation: u32) -> Self {
        TenantHandle { index, generation }
    }

    /// The tenant-pool index this handle names.
    pub fn index(self) -> u32 {
        self.index
    }

    /// The generation of the occupant this handle was minted for.
    pub fn generation(self) -> u32 {
        self.generation
    }
}

/// A handle to any kind of cell: a place — a slab slot, a tree-pool index or a tenant-pool index —
/// paired with the generation its occupant held when the handle was minted. Exactly the three
/// things a step can run in, and so the three a step can name as a placement destination, a parent
/// or a host.
///
/// Two of the three own a region. A tenant owns none and writes its host's, so where a door asks
/// for a place with storage, a live tenant's name resolves to its host — whatever the host's own
/// life state, since a host cannot dispose while a tenant is counted on it. A tenant is therefore
/// never itself a host, a parent or a home.
///
/// A place and a generation is the whole of what the kinds share, and it is the line the name
/// draws against the other tier. There is no sealed variant, because a sealed cell occupies no
/// place and carries no generation: its id is drawn once and never re-bound, so there is nothing
/// for a handle to re-check, and [`SealedId`](crate::sealed::SealedId) names it instead. Nothing is
/// minted into one, it is never entered, and it parents nothing, so an embedder never holds its id
/// at all. "Root" and "host" are roles rather than kinds — any slab cell becomes a root the moment
/// it parents a tree cell, any region-owning cell a host the moment a tenant is created on it, and
/// nothing about it changes.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum CellHandle {
    Slab(SlabHandle),
    Tree(TreeHandle),
    Tenant(TenantHandle),
}

impl From<SlabHandle> for CellHandle {
    fn from(handle: SlabHandle) -> Self {
        CellHandle::Slab(handle)
    }
}

impl From<TreeHandle> for CellHandle {
    fn from(handle: TreeHandle) -> Self {
        CellHandle::Tree(handle)
    }
}

impl From<TenantHandle> for CellHandle {
    fn from(handle: TenantHandle) -> Self {
        CellHandle::Tenant(handle)
    }
}

/// A name for a cell that owns a region — the two kinds a value can be **homed** in. What a
/// dormant carrier's key and a tree tombstone's forward are typed at, so neither can name a
/// tenant: a tenant has no storage for a value to rest in or for bytes to splice into.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum HomeHandle {
    Slab(SlabHandle),
    Tree(TreeHandle),
}

impl From<HomeHandle> for CellHandle {
    fn from(handle: HomeHandle) -> Self {
        match handle {
            HomeHandle::Slab(handle) => CellHandle::Slab(handle),
            HomeHandle::Tree(handle) => CellHandle::Tree(handle),
        }
    }
}

/// A name whose occupant has died: the slot or pool index is free, holds a later generation, or
/// holds a cell whose death the embedder already declared. A stale name is always an error and
/// never a silent no-op, because it means a caller kept a name past a death it declared itself.
///
/// `N` is the name that went stale, and a door's error is exactly as wide as the name it takes:
/// [`SlabHandle`] from a door that names only a slab cell, [`TreeHandle`] or [`TenantHandle`] from
/// one that names only that kind, and [`CellHandle`] from one that takes any — so no door reports a kind it cannot
/// have met.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Stale<N>(pub(crate) N);

impl<N> Stale<N> {
    /// The name that went stale — the one the caller kept past a death it declared itself.
    pub fn name(self) -> N {
        self.0
    }
}

impl From<Stale<SlabHandle>> for Stale<CellHandle> {
    fn from(stale: Stale<SlabHandle>) -> Self {
        Stale(CellHandle::Slab(stale.0))
    }
}

impl From<Stale<TreeHandle>> for Stale<CellHandle> {
    fn from(stale: Stale<TreeHandle>) -> Self {
        Stale(CellHandle::Tree(stale.0))
    }
}

impl From<Stale<TenantHandle>> for Stale<CellHandle> {
    fn from(stale: Stale<TenantHandle>) -> Self {
        Stale(CellHandle::Tenant(stale.0))
    }
}
