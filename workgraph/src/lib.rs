//! The witnessed carrier substrate and the workload-generic DAG scheduler — Koan-agnostic by
//! construction, since this crate depends on nothing above it and so cannot name an embedder
//! type. Koan is the first embedder: it re-exports both modules from its own crate root
//! (`koan::witnessed`, `koan::scheduler`) so its internal `crate::witnessed::…` /
//! `crate::scheduler::…` paths keep resolving unchanged.
//!
//! ## Public surface
//!
//! [`witnessed`] — the lifetime-erasure carrier substrate:
//! - The carrier types: [`witnessed::Witnessed`], [`witnessed::Sealed`],
//!   [`witnessed::SealedExtern`], and the raw-retype currency [`witnessed::Erased`].
//! - The witness traits an embedder implements for its own region-owner type:
//!   [`witnessed::Witness`], [`witnessed::WitnessRegion`], [`witnessed::RegionOwner`] (the
//!   `Rc<F>` blanket-impl seam for [`witnessed::WitnessRegion`]), and the reference-only
//!   composition seam [`witnessed::ComposeWitness`].
//! - The opaque reach-set library type [`witnessed::RegionSet`], generic over the member trait
//!   [`witnessed::PinsRegion`] an embedder implements for its own frame-owner type.
//! - The lifetime family contract: [`witnessed::Reattachable`] and the
//!   [`witnessed::reattachable`] macro that discharges its `unsafe` obligation once per family.
//! - The generic region engine: [`witnessed::Region`], [`witnessed::StorageProfile`],
//!   [`witnessed::Stored`] (an embedder's storage-policy extension point).
//! - Combinators: [`witnessed::seal_option`], and the `And` / `OptionOf` families the `zip` /
//!   `seal_option` combinators seal.
//! - [`witnessed::doctest_fixture`] — a fixture crate for the `compile_fail` soundness guards;
//!   not part of the real surface (see its own module docs).
//!
//! [`scheduler`] — the workload-generic DAG scheduler:
//! - [`scheduler::Scheduler`], generic over an embedder's [`scheduler::Workload`] impl.
//! - [`scheduler::Live`], [`scheduler::Deps`] / [`scheduler::DepResults`] /
//!   [`scheduler::ResolvedDeps`], [`scheduler::ProducerDisposition`], [`scheduler::NodeId`].
//! - [`scheduler::nodes`]'s [`scheduler::nodes::Node`], [`scheduler::nodes::NodeFrame`],
//!   [`scheduler::nodes::NodeWork`] — the generic per-node state the scheduler stores.
//! - A `test-hooks` cargo feature widens a white-box surface (slot/edge state pokes: e.g.
//!   `Scheduler::clear_node`, `Scheduler::set_dep_edges`) from `cfg(test)` to
//!   `cfg(any(test, feature = "test-hooks"))`, so an embedder's own white-box tests — compiled
//!   as a dependent crate, where `cfg(test)` is off — can still reach it.

pub mod scheduler;
pub mod witnessed;
