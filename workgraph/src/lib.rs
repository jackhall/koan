//! A deferred-work DAG scheduler over a witnessed region-memory substrate. The crate depends on
//! nothing above it and so cannot name an embedder type — workload-genericity is structural here,
//! not a convention. Koan is the first embedder, re-exporting both modules from its own crate root
//! (`koan::witnessed`, `koan::scheduler`).
//!
//! [`witnessed`] is the lifetime-erasure carrier substrate
//! ([design/witnessed-memory.md](../design/witnessed-memory.md)). [`witnessed::Witnessed`] bundles
//! a value whose borrow lifetime is erased to `'static` with the liveness witness pinning its
//! pointee, so "the witness keeps the value alive" is a type invariant rather than a comment. One
//! value family moves through three states connected by transform verbs rather than by wrapping:
//! [`witnessed::Delivered`] in transit (owned pins), [`witnessed::Sealed`] at rest (weak members
//! via an arena-hosted description), and [`witnessed::Opened`] in use (borrowed at a step lifetime
//! under a presented pin, which is what a membership query is answered against).
//!
//! Reach evidence splits by ownership ([design/reach.md](../design/reach.md)):
//! [`witnessed::ReachDescription`] is non-owning and side-table hosted, while
//! [`witnessed::StepCoverage`] is owned and deliberately a *narrowed* view — the pin arithmetic
//! behind it is crate-private, so an embedder can name coverage and widen it but cannot assemble
//! or narrow a claim by hand. A region *interns* descriptions, so within one region a
//! description's address is its member set, and an entry's existence is proof the region already
//! pins what it names. Sub-value granularity lives in [`witnessed::Sectioned`]
//! ([design/sectioned-reach.md](../design/sectioned-reach.md)).
//!
//! Construction is doored: a value and the description it is sealed under come off the same
//! [`witnessed::RegionHandle`] ([`witnessed::RegionHandle::mint_retained`],
//! [`witnessed::RegionHandle::seal_reaching`]), so no embedder-reachable path pairs a loose value
//! with a loose witness. The lifetime-retype `unsafe` obligation is discharged once per carrier
//! family through the [`witnessed::reattachable`] macro. An embedder plugs in by implementing the
//! witness traits for its own region-owner type ([`witnessed::Witness`],
//! [`witnessed::WitnessRegion`], [`witnessed::RegionOwner`]) and declaring its frame-owner type
//! through [`witnessed::StorageProfile`].
//!
//! [`scheduler`] is the workload-generic DAG scheduler
//! ([design/dag-scheduler.md](../design/dag-scheduler.md)). [`scheduler::Scheduler`] is generic
//! over an embedder's [`scheduler::Workload`] impl and runs through one door,
//! [`scheduler::Scheduler::drain`]: the embedder's step callback receives a [`scheduler::Step`]
//! and returns a [`scheduler::StepVerdict`] the drain applies, and deadlock surfaces as
//! [`scheduler::DrainDeadlock`].
//!
//! The `test-hooks` cargo feature widens both modules' white-box surface (slot/edge state pokes,
//! the doctest fixture) from `cfg(test)` to `cfg(any(test, feature = "test-hooks"))`, so an
//! embedder's own white-box tests — compiled as a dependent crate, where `cfg(test)` is off — can
//! still reach it, while a production build has no such surface to accrete against.

pub mod scheduler;
#[cfg(test)]
mod tests;
pub mod witnessed;
