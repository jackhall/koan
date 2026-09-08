//! Cell identity, over both habitats: a slab slot or a tree-pool index, paired with the generation
//! of the occupant that place held when the handle was minted. [`CellRef`] is the two of them as
//! one name, which is what a door that takes a destination or a parent asks for. See
//! [design/cellgraph.md](../design/cellgraph.md) § The cell and
//! [design/tree-cells.md](../design/tree-cells.md).

/// A name for one cell: the slab slot it occupies, plus the generation that distinguishes it from
/// every other occupant of that slot. `Copy`, so a handle is passed around freely; naming a cell
/// grants nothing, because every verb re-checks the generation against the slot.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Handle {
    slot: u32,
    generation: u32,
}

impl Handle {
    pub(crate) fn new(slot: u32, generation: u32) -> Self {
        Handle { slot, generation }
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
/// from every other occupant of that index. `Copy`, like [`Handle`], and re-checked by every verb
/// for the same reason.
///
/// The pool is separate from the slab and takes no cap, so a tree handle names no slab bit and no
/// mask ever holds one. See [design/tree-cells.md](../design/tree-cells.md).
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

/// Either kind of cell, by name — exactly the two things a step can name as a placement
/// destination, a parent, or the cell it is running in.
///
/// There is no sealed variant, because a sealed region is not named this way: it has no generation
/// — its id is drawn once and never re-bound — so there is nothing for a handle to re-check, and
/// [`SealedId`](crate::sealed::SealedId) names it instead. Nothing is minted into one, it is never
/// entered, and it parents nothing, so an embedder never holds its id at all. "Root" is a role
/// rather than a kind — any slab cell becomes one the moment it parents a tree cell, and nothing
/// about it changes.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum CellRef {
    Slab(Handle),
    Tree(TreeHandle),
}

impl From<Handle> for CellRef {
    fn from(handle: Handle) -> Self {
        CellRef::Slab(handle)
    }
}

impl From<TreeHandle> for CellRef {
    fn from(handle: TreeHandle) -> Self {
        CellRef::Tree(handle)
    }
}

/// A name whose occupant has died: the slot or pool index is free, holds a later generation, or
/// holds a cell whose death the embedder already declared. A stale name is always an error and
/// never a silent no-op, because it means a caller kept a name past a death it declared itself.
///
/// `N` is the name that went stale, and a door's error is exactly as wide as the name it takes:
/// [`Handle`] from a door that names only a slab cell, [`TreeHandle`] from one that names only a
/// tree cell, and [`CellRef`] from one that takes either — so no door reports a kind it cannot
/// have met.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Stale<N>(pub(crate) N);

impl<N> Stale<N> {
    /// The name that went stale — the one the caller kept past a death it declared itself.
    pub fn name(self) -> N {
        self.0
    }
}

impl From<Stale<Handle>> for Stale<CellRef> {
    fn from(stale: Stale<Handle>) -> Self {
        Stale(CellRef::Slab(stale.0))
    }
}

impl From<Stale<TreeHandle>> for Stale<CellRef> {
    fn from(stale: Stale<TreeHandle>) -> Self {
        Stale(CellRef::Tree(stale.0))
    }
}
