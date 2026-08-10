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
//! - One value family in three states, connected by transform verbs rather than by wrapping:
//!   [`witnessed::Delivered`] in transit (owned pins), [`witnessed::Sealed`] at rest (weak members
//!   via an arena-hosted description), and [`witnessed::Opened`] in use (borrowed at a step
//!   lifetime under a presented pin — the only state that answers a membership query). The
//!   reference-only reach witness they carry is [`witnessed::Carrier`].
//! - The witness traits an embedder implements for its own region-owner type:
//!   [`witnessed::Witness`], [`witnessed::WitnessRegion`], [`witnessed::RegionOwner`] (the
//!   `Rc<F>` blanket-impl seam for [`witnessed::WitnessRegion`]), and the reference-only
//!   composition seam [`witnessed::ComposeWitness`].
//! - The reach-evidence types [`witnessed::ReachDescription`] (non-owning, side-table hosted) and
//!   [`witnessed::PinBundle`] (owned), generic over the member trait [`witnessed::PinsRegion`] an
//!   embedder implements for its own frame-owner type. Both are frozen together at a
//!   [`witnessed::ReachDescription::mint_resident`], which is also where the destination's retention
//!   of the composed bundle is established, and a value's home region rides them as an ordinary
//!   member — the sole asymmetry is the self rule, which strips `dest`'s own region from the
//!   retained bundle (a region pinning itself is a cycle) while leaving it in the description. A
//!   region's side table *interns* descriptions, so within one region a description's address is
//!   its member set — and an entry's existence is proof the region already pins what it names.
//! - Sub-value reach storage: [`witnessed::Sectioned`], a cell-generic `Copy`, `Drop`-free container
//!   whose cells are physically partitioned into contiguous runs each naming one interned
//!   description — mapping and partition both bumped into the region, so a teardown never walks one — built
//!   through the single alloc door [`witnessed::Sectioned::build`] over per-cell
//!   [`witnessed::CellInput`] / [`witnessed::CellReach`] verdicts.
//!   [`witnessed::Sectioned::project`] parts a cell as a bundled
//!   `Opened<'a, CellRef<K>, Carrier<F>>` — the cell reference and exactly its run's reach in one
//!   `'a`-confined value ([`witnessed::CellRef`]), never as loose parts.
//! - The lifetime family contract: [`witnessed::Reattachable`] and the
//!   [`witnessed::reattachable`] macro that discharges its `unsafe` obligation once per family.
//! - The generic region engine: [`witnessed::Region`], [`witnessed::StorageProfile`],
//!   [`witnessed::StorageProfile`] (an embedder's storage declaration).
//! - Combinators: [`witnessed::seal_option`], and the `And` / `OptionOf` families the `zip` /
//!   `seal_option` combinators seal.
//! - [`witnessed::doctest_fixture`] — a fixture crate for the `compile_fail` soundness guards;
//!   not part of the real surface (see its own module docs).
//!
//! [`scheduler`] — the workload-generic DAG scheduler:
//! - [`scheduler::Scheduler`], generic over an embedder's [`scheduler::Workload`] impl.
//! - [`scheduler::Live`], [`scheduler::Deps`] / [`scheduler::DepResults`] /
//!   [`scheduler::ResolvedDeps`], [`scheduler::NodeId`].
//! - [`scheduler::nodes`]'s [`scheduler::nodes::NodeWork`] — the generic per-node work the scheduler
//!   stores, paired with the per-slot memory anchor ([`scheduler::Anchor`]) it holds by `Rc`.
//! - A `test-hooks` cargo feature widens a white-box surface (slot/edge state pokes: e.g.
//!   `Scheduler::clear_node`, `Scheduler::set_dep_edges`) from `cfg(test)` to
//!   `cfg(any(test, feature = "test-hooks"))`, so an embedder's own white-box tests — compiled
//!   as a dependent crate, where `cfg(test)` is off — can still reach it.

pub mod scheduler;
pub mod witnessed;
