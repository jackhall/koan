//! Where a value lives and how long — Koan's instantiation of the region substrate, and every
//! substrate name Koan spells.
//!
//! The payload-generic engine underneath is `workgraph`'s witnessed module
//! ([workgraph/design/witnessed-memory.md](../workgraph/design/witnessed-memory.md)); this module is
//! Koan's policy over it. [`region`] declares the storage profile and the allocation brands,
//! [`frame`] the per-call frame shell, [`program`] the program-text tier above the run root, and
//! [`container_substrate`] / [`rehomed`] the region-resident container storage.
//!
//! **Every substrate name Koan spells is an item here.** [`substrate`] holds one Koan-bound alias
//! per library generic — `Delivered<T>`, `Sealed<'h, T>`, `RegionHandle<'a>`, `FoldedPlacement<'b>`
//! and the rest, each binding Koan's witness, owner and profile — plus a verbatim re-export of the
//! names that take no Koan parameter. Outside this module no file names `workgraph`, `hashbrown` or
//! `allocator_api2`; swapping the substrate is a rewrite of [`substrate`] and of
//! [`machine::execute::step`](crate::machine::execute::step), which owns the step brand because
//! `StepCarried`'s only exit is confined to `execute`.
//!
//! **An alias is not an instantiation.** A name that binds a *payload* — `Delivered<CarriedFamily>`,
//! `Sealed<'h, KFunctionFamily>` — belongs with that payload, not here: the value cells and their
//! carrier states live in [`values::cell`](crate::machine::model::values), the callable's in
//! [`core::kfunction`](crate::machine::core), the operator group's in
//! [`model::operators`](crate::machine::model::operators). Each is one line applying a [`substrate`]
//! alias to a local family, so the payload's own file is where a reader finds every state it travels
//! in.
//!
//! **What this module imports back.** [`frame`] names [`Scope`](crate::machine::core::Scope) to
//! read the child a frame's envelope carries — the payload of the family it holds — and the
//! container storage names `Held`, `KObject`, `KKey` and `Symbol` for the same reason. [`region`]
//! names nothing but the lifetime-free [`KType`](crate::machine::model::KType) handle its
//! `RegionTypeFamily` carries. Nothing from `machine::execute`, nothing that builds a value (a
//! frame's child scope is born by [`Scope::open_frame`](crate::machine::core::Scope::open_frame),
//! which hands this module the finished pair), and nothing of the run's own state — the registries,
//! the interner and the output sink belong to
//! [`execute::RunFrame`](crate::machine::execute). That list is the inventory a future
//! payload-family inversion would work from; anything not on it is a new edge, not a detail.
//!
//! See [memory-model.md](../design/memory-model.md),
//! [value-substrates.md](../design/value-substrates.md) and
//! [per-call-region/](../design/per-call-region/README.md).

pub mod container_substrate;
pub mod frame;
pub mod program;
pub mod region;
pub mod rehomed;
pub mod substrate;

#[cfg(test)]
mod tests;

pub use container_substrate::{ContainerSubstrate, PartedCell};
pub(crate) use container_substrate::{
    DictSubstrate, ListSubstrate, PayloadSubstrate, RecordSubstrate, object_copy_cost,
};
pub use frame::{CallFrame, FrameCoverage, FrameReach};
pub use program::{ProgramBrand, ProgramStorage, program_storage};
pub use region::{
    FoldingBrand, FrameStorage, KoanRegion, RegionBrand, RegionTypeFamily, SubstrateDoor,
    run_root_storage,
};
pub(crate) use region::{FrameStorageExt, KoanRegionExt, KoanStorageProfile, bump_table};
pub(crate) use rehomed::Rehomed;
pub use substrate::*;
