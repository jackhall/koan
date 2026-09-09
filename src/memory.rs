//! Where a value lives and how long — Koan's instantiation of the region substrate, and every
//! substrate name Koan spells.
//!
//! The payload-generic engine underneath is `workgraph`'s witnessed module
//! ([workgraph/design/witnessed-memory.md](../workgraph/design/witnessed-memory.md)); this module is
//! Koan's policy over it. [`region`] declares the storage profile and the allocation brands,
//! [`frame`] the per-call frame shell, [`program`] the program-text tier above the run root,
//! [`carrier`] the states a value's carrier rests and travels in, [`cell`] the value-channel cells
//! themselves, and [`container_substrate`] / [`rehomed`] the region-resident container storage.
//!
//! **Every substrate name Koan spells is an item here.** [`substrate`] holds one Koan-bound alias
//! per library generic — `Delivered<T>`, `Sealed<'h, T>`, `RegionHandle<'a>`, `FoldedPlacement<'b>`
//! and the rest, each binding Koan's witness, owner and profile — plus a verbatim re-export of the
//! names that take no Koan parameter. Outside this module no file names `workgraph`, `hashbrown` or
//! `allocator_api2`; swapping the substrate is a rewrite of [`substrate`] and of
//! [`machine::execute::step`](crate::machine::execute::step), which owns the step brand because
//! `StepCarried`'s only exit is confined to `execute`.
//!
//! **What this module imports back.** The cells name `KObject` concretely rather than being
//! parameterised over a payload family, so `memory` depends on `machine::core::{scope, scope_id,
//! kfunction}` and `machine::model::{values::kobject, values::kkey, labels, ast, operators}`, plus
//! `machine::model::types::KType` (a lifetime-free handle) and `RunRegistries` (a field of the run
//! [`CallFrame`]). Nothing from `machine::execute`. That list is the inventory a future inversion
//! would work from; anything not on it is a new edge, not a detail.
//!
//! See [memory-model.md](../design/memory-model.md),
//! [value-substrates.md](../design/value-substrates.md) and
//! [per-call-region/](../design/per-call-region/README.md).

pub mod carrier;
pub mod cell;
pub mod container_substrate;
pub mod frame;
pub mod program;
pub mod region;
pub mod rehomed;
pub mod substrate;

#[cfg(test)]
mod tests;

pub use carrier::{
    CarrierWitness, DeliveredCarried, DeliveredFunction, DeliveredOperatorGroup, OpenedFunction,
    SealedFunction, SealedOperatorGroup, SplicedCell,
};
pub(crate) use carrier::{product_reaches_region, read_resting};
pub use cell::{Carried, CarriedFamily, Held};
pub use container_substrate::{ContainerSubstrate, PartedCell};
pub(crate) use container_substrate::{
    DictSubstrate, ListSubstrate, PayloadSubstrate, RecordSubstrate, object_copy_cost,
};
pub use frame::{CallFrame, FrameCoverage, FrameReach, RunWriter};
pub use program::{ProgramBrand, ProgramStorage, program_storage};
pub use region::{
    FoldingBrand, FrameStorage, KoanRegion, RegionBrand, RegionTypeFamily, SubstrateDoor,
    run_root_storage,
};
pub(crate) use region::{FrameStorageExt, KoanRegionExt, KoanStorageProfile, bump_table};
pub(crate) use rehomed::Rehomed;
pub use substrate::*;
