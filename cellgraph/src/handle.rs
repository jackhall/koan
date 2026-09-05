//! Cell identity: a slab slot paired with the generation of the occupant that slot held when the
//! handle was minted. See [design/cellgraph.md](../design/cellgraph.md) § The cell.

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
pub struct StaleHandle(pub Handle);
