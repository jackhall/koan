//! Every substrate name Koan spells, in one place: the `workgraph` region library's types under
//! Koan's own spelling. A name arrives here one of two ways — re-exported verbatim when the library
//! type takes no Koan parameter (the bump doors, the family contract, the erase primitive), or as a
//! **Koan-bound alias** when it does, binding Koan's witness ([`CarrierWitness`]), owner
//! ([`FrameStorage`]) and storage profile ([`KoanStorageProfile`]) once so no use site restates them.
//!
//! This is the crate's only import of `workgraph::witnessed`, `hashbrown` and `allocator_api2`: the
//! substrate reaches Koan through this file and [`region`](super::region)'s bump-backed table
//! constructor, and every other module names a `crate::memory` item. Swapping the substrate is
//! therefore a rewrite of this file and of
//! [`machine::execute::step`](crate::machine::execute::step), the one seam outside `memory` that
//! spells the region substrate's own brand doors.
//!
//! One alias per library generic, and no second alias for the same generic: a site that needs a
//! parameter an alias does not bind is a design question about the profile, not a variant to add
//! here. See [design/scheduler-library.md](../../design/scheduler-library.md) for the crate
//! boundary this file sits on, and
//! [workgraph/design/witnessed-memory.md](../../workgraph/design/witnessed-memory.md) for the
//! machinery itself.

use super::region::{FrameStorage, KoanStorageProfile};

// The library shape, re-exported unchanged: these take no Koan parameter, so Koan's spelling is the
// library's.
pub use workgraph::witnessed::{
    And, BumpAllocator, BumpBackedMap, BumpVec, Carrier, CellRef, DropFree, HasRegionHandle,
    PinsRegion, ReachDescription, Reattachable, ReferenceFamily, Region, RegionHost, SealedExtern,
    StepCoverage, StorageProfile, Within, WitnessRegion, erase_to_static, reattachable,
};

/// The custom-allocator `Vec` [`BumpVec`] is an instance of, with the allocator trait and the
/// global-heap allocator it defaults to. A site generic over *which* arena its scratch buffer sits
/// in (a dispatch probe filling the step scratch, the block-entry id run) names the open form here
/// rather than importing the allocator crate itself.
pub use allocator_api2::alloc::{Allocator, Global};
pub use allocator_api2::vec::Vec as AllocVec;

/// The region-turnover counters — the numbers a tail-loop test reads to prove a retiring frame's
/// region is freed rather than chained. Present only where the library's white-box readers are
/// compiled in: the lib-test build, or a `region-audit` interpreter run.
#[cfg(any(test, feature = "region-audit"))]
pub use workgraph::witnessed::{RegionMetrics, region_metrics, reset_region_metrics};

/// The pin-ring detector's reports — a debug-build diagnostic the interpreter binary prints after a
/// run, compiled out of a release build along with the detector itself.
#[cfg(debug_assertions)]
pub use workgraph::witnessed::{PinCycleReport, pin_cycle_reports};

/// Koan's value-carrier witness: the library [`Carrier`] over Koan's frame owner — a reference to
/// the value's hosted reach description and nothing else. The description carries both of the
/// value's region facts: its *host* is the region the value lives in, its *members* are the regions
/// the value's borrows reach, home among them exactly when the value genuinely borrows into its own
/// region. The carrier pins nothing; liveness is always the containing region — the producer's own
/// while the delivery walk carries the terminal, the destination's the moment the walk adopts it in.
///
/// Every alias below binds this as the `W` parameter, which is why no use site spells it: a site
/// that constructs or inspects a carrier routes the library's [`Carrier`] surface directly.
pub type CarrierWitness = Carrier<FrameStorage>;

/// A value carrier **in transit**, with its whole reach owned: the library envelope over Koan's
/// carrier witness, pinned by a [`FrameStorage`] home.
pub type Delivered<T> = workgraph::witnessed::Delivered<T, CarrierWitness, FrameStorage>;

/// A [`Delivered`] minus its home pin — a carrier fused to its owned coverage, held by a producer
/// that does not yet know which frame will own it. Its only exit supplies that pin.
pub type Unhosted<T> = workgraph::witnessed::Unhosted<T, CarrierWitness, FrameStorage>;

/// A carrier **at rest** in the region hosting its description, branded by that region's `'home`, so
/// reading it takes no pin of its own.
pub type Sealed<'home, T> = workgraph::witnessed::Sealed<'home, T, CarrierWitness>;

/// A carrier **in use**: re-anchored at a brand lifetime `'b`, paired with the reach witness it was
/// opened under.
pub type Opened<'b, T> = workgraph::witnessed::Opened<'b, T, CarrierWitness>;

/// The lifetime-free bundle of an erased value with its reach witness — the storable form every
/// carrier state is built from.
pub type Witnessed<T> = workgraph::witnessed::Witnessed<T, CarrierWitness>;

/// A carrier whose home liveness is a refcount rather than a lifetime — the state between a seal
/// and the lift that gives it an owning envelope.
pub type Retained<T> = workgraph::witnessed::Retained<T, CarrierWitness>;

/// A carrier resting under a pin held elsewhere, for the sites whose liveness argument is the
/// holder's rather than the description's.
pub type SealedPinned<T> = workgraph::witnessed::SealedPinned<T, CarrierWitness>;

/// The library's allocation capability over a Koan region, at a region-borrow lifetime `'a`. Koan's
/// typed veneer over it is [`RegionBrand`](super::region::RegionBrand); a bare handle crosses a
/// construction brand as the handle-headed operand families' head.
pub type RegionHandle<'a> = workgraph::witnessed::RegionHandle<'a, KoanStorageProfile>;

/// The [`Reattachable`] family of a bare [`RegionHandle`] — the destination operand of a
/// construction that builds into a region it is handed.
pub type RegionHandleFamily = workgraph::witnessed::RegionHandleFamily<KoanStorageProfile>;

/// The fold-brand placement capability: proof, minted by a fold engine over the destination region
/// and confined to the closure by `'b`, that a value built here is covered by the fold's composition.
pub type FoldedPlacement<'b> = workgraph::witnessed::FoldedPlacement<'b, KoanStorageProfile>;

/// A container's cell storage: cells in semantic order, physically partitioned into runs that each
/// name one interned reach description.
pub type Sectioned<'a, K> = workgraph::witnessed::Sectioned<'a, K, FrameStorage>;

/// One cell's reach verdict as the sectioned build door takes it — owned, or pinned by a named
/// description.
pub type CellReach<'r> = workgraph::witnessed::CellReach<'r, FrameStorage>;

/// One cell handed to the sectioned build door: the payload paired with its [`CellReach`] verdict.
pub type CellInput<'a, 'r, K> = workgraph::witnessed::CellInput<'a, 'r, K, FrameStorage>;

/// The library construction context over a step's destination frame — what
/// [`StepAllocator`](crate::machine::execute::step::StepAllocator) and the zero-dep fold door
/// allocate through.
pub type StepContext = workgraph::witnessed::StepContext<FrameStorage>;
