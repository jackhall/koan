//! Where a value lives and how long — Koan's instantiation of the region substrate, and every
//! substrate name Koan spells.
//!
//! The payload-generic engine underneath is `workgraph`'s witnessed module
//! ([workgraph/design/witnessed-memory.md](../workgraph/design/witnessed-memory.md)); this module is
//! Koan's policy over it. [`region`] declares the storage profile and the allocation brands,
//! [`frame`] the per-call frame shell, and [`program`] the program-text tier above the run root.
//!
//! **Every substrate name Koan spells is an item here.** [`substrate`] holds one Koan-bound alias
//! per library generic — `Delivered<T>`, `Sealed<'h, T>`, `RegionHandle<'a>`, `FoldedPlacement<'b>`
//! and the rest, each binding Koan's witness, owner and profile — plus a verbatim re-export of the
//! names that take no Koan parameter. Outside this module no file names `workgraph`, `hashbrown` or
//! `allocator_api2`; swapping the substrate is a rewrite of [`substrate`] and of
//! [`machine::execute::step`](crate::machine::execute::step), which owns the step brand because
//! `StepCarried`'s only exit is confined to `execute`.
//!
//! **An alias is not an instantiation, and a store is not the storage.** A name that binds a
//! *payload* — `Delivered<CarriedFamily>`, `Sealed<'h, KFunctionFamily>` — belongs with that
//! payload: the value cells and their carrier states live in
//! [`values::cell`](crate::machine::model::values), the callable's in
//! [`core::kfunction`](crate::machine::core), the operator group's in
//! [`model::operators`](crate::machine::model::operators). So does anything shaped by what it
//! holds: the container substrates and the rehoming door are `Held`-cell storage, and sit in
//! [`values`](crate::machine::model::values) beside the cells. Each is one line applying a
//! [`substrate`] alias, or one type over the [`Sectioned`] run the library hands it — so the
//! payload's own file is where a reader finds every state it travels in.
//!
//! **What this module imports back — one name.** [`frame`] names `Scope` (with `ScopeId` and
//! `ScopeRefFamily`) to read the child a frame's envelope carries, which is the payload of the
//! family it holds. That is the whole inventory: [`region`], [`program`] and [`substrate`] import
//! nothing from the rest of Koan at all. No Koan value type is named here; nothing here *builds*
//! one — a frame's child scope is born by
//! [`Scope::open_frame`](crate::machine::core::Scope::open_frame), which hands this module the
//! finished pair, and a construction operand over a region handle belongs with the constructor
//! that mints it; and none of the run's own state lives here — the registries, the interner and
//! the output sink belong to [`execute::RunFrame`](crate::machine::execute). Anything not on that
//! list is a new edge, not a detail.
//!
//! See [memory-model.md](../design/memory-model.md),
//! [value-substrates.md](../design/value-substrates.md) and
//! [per-call-region/](../design/per-call-region/README.md).

pub mod frame;
pub mod program;
pub mod region;
pub mod substrate;

#[cfg(test)]
mod tests;

pub use frame::{CallFrame, FrameCoverage, FrameReach};
pub use program::{ProgramBrand, ProgramStorage, program_storage};
pub use region::{
    FoldingBrand, FrameStorage, KoanRegion, RegionBrand, SubstrateDoor, run_root_storage,
};
pub(crate) use region::{FrameStorageExt, KoanRegionExt, KoanStorageProfile, bump_table};
pub use substrate::*;
