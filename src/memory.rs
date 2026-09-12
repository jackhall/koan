//! Where a value lives and how long — the shape of things in storage, in two tiers.
//!
//! The **cell tier** is `cellgraph`'s: a call's storage is a cell, and what rests in it is laid
//! down through the cell's [`Writer`](crate::memory::Writer). [`substrate`](crate::memory::substrate) re-exports the library under Koan's spelling
//! and binds its width; [`SlotArray`](crate::memory::SlotArray) is the one shape here built in a cell's region.
//!
//! The **bump tier** is storage outside the graph — [`Bump`](crate::memory::Bump), [`BumpAllocator`](crate::memory::BumpAllocator), [`BumpVec`](crate::memory::BumpVec) and
//! [`BumpBackedMap`](crate::memory::BumpBackedMap) — for the AST ([`program`](crate::memory::program)) and the type lattice's registry and scratch.
//!
//! [`scope_id`](crate::memory::scope_id) is the position-independent identity a resident carries so nothing depends on where
//! it sits.
//!
//! **Imports.** Outside this module no file names `cellgraph`, `bumpalo`, `hashbrown` or
//! `allocator_api2`, and this module names no item from the rest of Koan outside doc comments.
//!
//! See [memory/README.md](memory/README.md).

mod bump;
pub mod program;
pub mod scope_id;
mod slots;
pub mod substrate;

#[cfg(test)]
mod tests;

pub(crate) use bump::bump_table;
pub use bump::{Bump, BumpAllocator, BumpBackedMap, BumpVec};
pub use program::{ProgramBrand, ProgramStorage, program_storage};
pub use scope_id::ScopeId;
pub use slots::{SlotArray, SlotConflict, SlotState};
pub use substrate::*;
