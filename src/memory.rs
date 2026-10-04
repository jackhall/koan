//! Where a value lives and how long — the shape of things in storage, in two tiers.
//!
//! The **cell tier** is `cellgraph`'s, and this module holds it: a call's storage is a cell, and
//! what rests in it is laid down through the cell's [`Writer`](crate::memory::Writer). [`substrate`](crate::memory::substrate) re-exports the library under Koan's spelling
//! and binds its width; [`SlotArray`](crate::memory::SlotArray) and [`Knot`](crate::memory::Knot) are the shapes here built in a cell's region.
//! [`program`](crate::memory::program) is program storage: one `cellgraph` store, written through a
//! [`Writer`](crate::memory::Writer) like a region.
//!
//! The **bump tier** is `lattice`'s — storage outside the graph, [`Bump`](crate::memory::Bump), [`BumpAllocator`](crate::memory::BumpAllocator), [`BumpVec`](crate::memory::BumpVec) and
//! [`BumpBackedMap`](crate::memory::BumpBackedMap), for the type lattice's registry and for the
//! scratch a caller passes, with the [`strongly_connected_components`](crate::memory::strongly_connected_components)
//! walk staged in it and the position-independent [`ScopeId`](crate::memory::ScopeId). It is
//! re-exported here, so koan names it as `memory`'s.
//!
//! **Imports.** No koan file outside this module names `cellgraph`, and this module names no item
//! from the rest of Koan outside doc comments.
//!
//! See [memory/README.md](memory/README.md).

mod knot;
pub mod program;
mod slots;
pub mod substrate;

#[cfg(test)]
mod tests;

pub use knot::{Edge, Knot, KnotPlan, Member};
pub use lattice::bump::{
    Bump, BumpAllocator, BumpBackedMap, BumpBackedSet, BumpVec, ScopeId,
    strongly_connected_components,
};
pub(crate) use lattice::bump::{bump_set, bump_table};
pub use program::{ProgramBrand, ProgramStorage, program_storage};
pub use slots::{SlotArray, SlotConflict, SlotView};
pub use substrate::*;
