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

/// A handle whose occupant has died: the slot is free, holds a later generation, or holds a cell
/// whose death the embedder already declared. A stale handle is always an error and never a silent
/// no-op, because it means a caller kept a name past a death it declared itself.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct StaleHandle(pub(crate) Handle);

impl StaleHandle {
    /// The handle that went stale — the name the caller kept past a death it declared itself.
    pub fn handle(self) -> Handle {
        self.0
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
/// There is no sealed variant: a record is not a cell. Nothing is minted into one, it is never
/// entered, and it parents nothing, so an embedder never holds its id. "Root" is a role rather than
/// a kind — any slab cell becomes one the moment it parents a tree cell, and nothing about it
/// changes.
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

/// A tree handle whose occupant has died — [`StaleHandle`]'s twin for the pool, and an error for
/// the same reason: the caller kept a name past a death it declared itself.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct StaleTree(pub(crate) TreeHandle);

impl StaleTree {
    /// The tree handle that went stale.
    pub fn handle(self) -> TreeHandle {
        self.0
    }
}

/// Either kind of stale name, for a door that takes a [`CellRef`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StaleCell {
    Slab(StaleHandle),
    Tree(StaleTree),
}

impl StaleCell {
    /// The name that went stale.
    pub fn cell(self) -> CellRef {
        match self {
            StaleCell::Slab(stale) => CellRef::Slab(stale.handle()),
            StaleCell::Tree(stale) => CellRef::Tree(stale.handle()),
        }
    }
}

impl From<StaleHandle> for StaleCell {
    fn from(stale: StaleHandle) -> Self {
        StaleCell::Slab(stale)
    }
}

impl From<StaleTree> for StaleCell {
    fn from(stale: StaleTree) -> Self {
        StaleCell::Tree(stale)
    }
}
