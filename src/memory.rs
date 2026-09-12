//! Where a value lives and how long — Koan's instantiation of the region substrate, and every
//! substrate name Koan spells.
//!
//! The payload-generic engine underneath is `workgraph::witnessed`'s; this module is Koan's
//! policy over it. [`region`] declares the storage profile and the allocation brands (with the
//! residence derivations a brand's region owner supplies), [`frame`] the per-call frame shell,
//! [`program`] the program-text tier above the run root, and [`scope_id`] the position-independent
//! identity a resident carries so nothing depends on where it sits.
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
//! **What this module imports back — nothing.** [`frame`], [`region`], [`program`], [`scope_id`]
//! and [`substrate`] name no item from the rest of Koan outside doc comments and `#[cfg(test)]`, so
//! `memory` is a leaf under the rest of the tree. The frame shell is generic over the family it
//! carries and its resident is built by the closure `core` hands
//! [`Frame::open_under`](frame::Frame::open_under) — which is how a Koan value gets into a frame
//! without this module naming one. Nothing here *builds* a Koan value: a construction operand over
//! a region handle belongs with the constructor that mints it. And none of the run's own state
//! lives here — the registries, the interner and the output sink belong to
//! [`execute::RunFrame`](crate::machine::execute); [`ScopeId`]'s counter is an identity source, not
//! a registry, and nothing is ever looked up against it. Anything not on that list is a new edge,
//! not a detail.
//!
//! **Storage shapes here, Koan vocabulary in `core`.** A payload-generic *shape* — [`BumpBackedMap`]
//! and the tables built over it, the frame shell, the layout-addressed [`SlotArray`] beside them —
//! belongs here, because what it holds does not enter its definition. The façade that instantiates
//! one at Koan's own vocabulary stays in [`core::bindings`](crate::machine::core::bindings):
//! `Bindings`, `BindingIndex`, the claim store and the write gate key on `model::labels` symbols
//! and hold `KType`, `SealedValue` and dispatch buckets, so moving them would import a dozen names
//! back. What is *storage* in a binding table is the table shape, and that already lives here.
//!
//! **The runtime is this module's consumer, and the runtime is `pending_rewrite`.** A door marked
//! `cfg_attr(not(feature = "pending_rewrite"), allow(dead_code))` has no caller in a default build
//! until the rewrite adopts it; the marker comes off with the adoption.
//!
//! See [memory/README.md](memory/README.md).

pub mod frame;
pub mod program;
pub mod region;
pub mod scope_id;
mod slots;
pub mod substrate;

#[cfg(all(test, feature = "pending_rewrite"))]
mod tests;

pub use frame::{Frame, FrameCoverage, FrameReach};
pub use program::{ProgramBrand, ProgramStorage, program_storage};
pub use region::{
    FoldingBrand, FrameStorage, KoanRegion, RegionBrand, SubstrateDoor, run_root_storage,
};
#[cfg_attr(not(feature = "pending_rewrite"), allow(unused_imports))]
pub(crate) use region::{FrameStorageExt, KoanRegionExt, KoanStorageProfile, bump_table};
pub use scope_id::ScopeId;
pub use slots::{SlotArray, SlotConflict, SlotState};
pub use substrate::*;
