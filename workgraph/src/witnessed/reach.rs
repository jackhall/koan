//! The reach-evidence types, split by ownership. A value's *reach* — the set of foreign region
//! owners its borrows keep alive — is carried by two cooperating types over one member trait
//! ([`PinsRegion`], the outer-chain subsumption hook a workload's frame-owner type supplies):
//!
//! - [`ReachDescription<F>`] — the **non-owning** description: `Weak<F>` members, hosted in a
//!   region's append-stable side table ([`Region::alloc_reach`]) and *referenced* (never owned) by
//!   a [`Carrier`](super::Carrier). It keeps nothing alive; it answers membership queries
//!   ([`Self::pins_region`] / [`Self::any_member_region`]) by upgrading its members under a pinned
//!   read. Whoever holds a description's coverage also holds the owning [`PinBundle`] that pins its
//!   members, so every upgrade succeeds under the holder rule — a failed upgrade is a coverage bug
//!   (`debug_assert` in debug, treated as non-pinning in release).
//! - [`PinBundle<F>`] — the **owned** witness: the antichain of `Rc<F>` owners a holder keeps to
//!   pin every member region. It plays the open-[`Witness`] role the description cannot, and
//!   releases its pins by ordinary `Drop` (a binding entry drops it at entry death, the delivery
//!   envelope carries it across the parked period).
//!
//! Mechanism (subsumption, folding, union) is library-owned; member semantics (what "pins" means
//! for a workload's frame type) is workload-supplied through [`PinsRegion`]. Both types are frozen
//! together at a [`ReachDescription::mint`] — the description into the destination's side table, the
//! owned bundle to the caller — from the source **bundles** the composition folds (strong `Rc`
//! members, never a description's `Weak`). A description is never upgraded to build an owned bundle:
//! ownership flows forward from a mint, threaded to every holder that needs pins. No constructor
//! builds either type from loose parts.
//!
//! The two outputs of a mint differ in exactly one member, the **self rule**
//! (design/witness-hosting.md § Composition): the description keeps every composed member,
//! `dest`'s own region included, so membership is exact — a value's home rides it as an ordinary
//! member rather than as a separate bit; the owned bundle drops any member whose region *is*
//! `dest`'s, because a region owning a pin on itself is a reference cycle. Ancestors of `dest`
//! stay in the bundle: they close no cycle. The asymmetry is what makes the `Sealed → Delivered`
//! lift correct for a value resting in its own scope's region — [`ReachDescription::to_bundle`]
//! upgrades home along with everything else when the value travels out.

use std::rc::{Rc, Weak};

use super::{
    ComposeWitness, Reattachable, Region, RegionHandle, RegionOwner, StorageProfile, Witness,
};

/// A [`RegionOwner`] that can report whether holding it keeps another region's storage alive — the
/// outer-chain subsumption hook [`PinBundle`] folds and inserts through, and the membership hook
/// [`ReachDescription`] answers queries with.
///
/// # Safety
///
/// `pins_region(r) == true` asserts that holding `Self` (behind its `Rc`) keeps the storage of the
/// region at `r` live and at a fixed address for as long as `Self` is held — `Self`'s own region or
/// one reached through an owner chain it pins. This is what makes subsumption sound: [`PinBundle`]
/// drops a member whose region another member already pins, and the remaining member must genuinely
/// carry that pin.
pub unsafe trait PinsRegion: RegionOwner {
    /// Whether holding `self` keeps the storage of `region` alive.
    fn pins_region(&self, region: &Self::Region) -> bool;
}

/// The non-owning reach description: the `Weak<F>` members naming the regions a carrier's value
/// reaches, hosted in the value's home region's own side table and referenced by the carrier. A
/// singleton for a single-region value (a lifted closure over one source region), larger for a
/// multi-region value, empty for a frameless / run-region terminal (encoded without an allocation
/// on the carrier). Holding a description pins **nothing**: its members are `Weak`, and the owning
/// [`PinBundle`] the holder keeps is what keeps them alive.
///
/// Queries ([`Self::pins_region`] / [`Self::any_member_region`]) run under a pinned read: they
/// upgrade each member, sound because whoever can reach this description holds the coverage that
/// keeps the members' `Rc`s alive (the holder rule). A member that fails to upgrade is a bug — a
/// read with no covering pin — so it `debug_assert`s and, in release, is treated as non-pinning.
pub struct ReachDescription<F: PinsRegion> {
    members: Vec<Weak<F>>,
}

impl<F: PinsRegion> ReachDescription<F> {
    /// The description mirror of an antichain of owners — `Weak` members for side-table hosting.
    /// The sole builder; a description's members always mirror some [`PinBundle`]'s antichain.
    fn from_members(members: &[Rc<F>]) -> Self {
        ReachDescription {
            members: members.iter().map(Rc::downgrade).collect(),
        }
    }

    /// Whether this description names no region — the frameless / run-region terminal.
    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    /// Upgrade each member under the holder rule and hand the live owner to `f`. A member that
    /// fails to upgrade is a coverage bug (a read with no pin covering it): `debug_assert` in debug,
    /// skipped (treated as non-pinning) in release.
    fn for_each_owner(&self, mut f: impl FnMut(&Rc<F>)) {
        for weak in &self.members {
            match weak.upgrade() {
                Some(owner) => f(&owner),
                None => debug_assert!(
                    false,
                    "ReachDescription member upgrade failed: read without a covering pin"
                ),
            }
        }
    }

    /// Whether any member's owner chain keeps `region`'s storage alive — the set-level lift of
    /// [`PinsRegion::pins_region`], run under a pinned read. The reach-covers query a carrier layers
    /// its finalize gate and bind-bit derivation on: "does this reach already name the region I'm
    /// about to fold?".
    pub fn pins_region(&self, region: &F::Region) -> bool {
        let mut hit = false;
        self.for_each_owner(|owner| {
            if !hit && owner.pins_region(region) {
                hit = true;
            }
        });
        hit
    }

    /// Whether any member's own region satisfies `pred` — a generalization of [`Self::pins_region`]
    /// for a caller with no single named target region to test (e.g. an address-table membership
    /// check against a raw stored pointer). No member reference escapes: `pred` runs against each
    /// upgraded member's region internally.
    pub fn any_member_region(&self, pred: impl Fn(&F::Region) -> bool) -> bool {
        let mut hit = false;
        self.for_each_owner(|owner| {
            if !hit && pred(owner.region()) {
                hit = true;
            }
        });
        hit
    }

    /// Upgrade every member `Weak → Rc` under a pinned read, collecting the owned [`PinBundle`] that
    /// pins each member region — the description-to-bundle upgrade the `Sealed → Delivered` **lift**
    /// routes (a value read out of an arena-hosted seal is re-owned so the source frame may die in
    /// transit). The one sanctioned production upgrade of a description into an owned bundle: it runs
    /// under the holder rule (the caller holds a pin covering this description's hosting arena for the
    /// whole call), so every member upgrade succeeds — a failure is the same coverage bug
    /// [`Self::for_each_owner`] `debug_assert`s. Subsumption is re-applied through
    /// [`PinBundle::insert`], so the result is an antichain of the deepest owners.
    pub fn to_bundle(&self) -> PinBundle<F> {
        let mut bundle = PinBundle::empty();
        self.for_each_owner(|owner| bundle.insert(Rc::clone(owner)));
        bundle
    }

    /// The description's live members, upgraded under a pinned read — white-box reach introspection.
    /// No library-internal caller, so gated entirely behind `test-hooks` for an embedder's own
    /// white-box tests (mirroring `Scheduler::anchor_of`'s gate).
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn members(&self) -> Vec<Rc<F>> {
        let mut members = Vec::new();
        self.for_each_owner(|owner| members.push(Rc::clone(owner)));
        members
    }

    /// Mint a frozen description into `dest`'s side table and return it alongside the owned
    /// [`PinBundle`] the caller keeps to pin its members (design/witness-hosting.md § Composition).
    /// Composes every source bundle in `sources` — its **exact** strong members, never "everything a
    /// region reaches" — applying exactly two rules, and no caller-supplied policy:
    ///
    /// 1. **Outer-chain subsumption** — via [`PinsRegion`], built into [`PinBundle::insert`]: a
    ///    member kept alive by another member's owner chain is dropped.
    /// 2. **The self rule**, applied to the returned **bundle only**: a member whose region *is*
    ///    `dest`'s is dropped from the owned bundle (a region owning a pin on itself is a cycle)
    ///    while staying a member of the stored description, so membership remains exact. Ancestors
    ///    of `dest` survive in both — they close no cycle.
    ///
    /// A minted description is therefore **exact**: it is the value's whole reach, not a reach
    /// narrowed against what some destination's container happened to pin, so every consumer reads
    /// the answer off it directly instead of re-deriving what a narrowing dropped.
    ///
    /// The fold runs over the sources' **strong** `Rc` members (no `Weak` upgrade — ownership flows
    /// forward from the mint, never recovered from a description), then the composed antichain is
    /// downgraded into the stored description, so the description's members mirror it. The returned
    /// `&'a` description is co-located in `dest`'s side table; the returned bundle is what keeps its
    /// members alive. `(None, empty bundle)` when the composed reach is empty — a region-pure value
    /// mints nothing, encoded without an allocation.
    pub fn mint<'a, W>(
        dest: RegionHandle<'a, W>,
        sources: &[&PinBundle<F>],
    ) -> (Option<&'a ReachDescription<F>>, PinBundle<F>)
    where
        // `W::FrameOwner = F` ties the destination's reach side table to this member type, so the
        // minted description lands in `dest`'s own [`Region::alloc_reach`] table. Binding `Region`
        // on `RegionOwner` (the trait that DECLARES it, not `PinsRegion`) avoids E0220 — a
        // supertrait's associated type is not bindable through the subtrait.
        W: StorageProfile<FrameOwner = F>,
        F: RegionOwner<Region = Region<W>>,
    {
        let mut composed = PinBundle::empty();
        for source in sources {
            for owner in &source.members {
                composed.insert(Rc::clone(owner)); // exact members + subsumption
            }
        }
        if composed.is_empty() {
            return (None, composed);
        }
        // Freeze the full antichain's mirror into the region's side table — membership is exact,
        // `dest`'s own region included — then strip the self member from the owned copy that rides
        // out to the caller (the binding entry, the delivery envelope, the region's own retention).
        let stored = dest.region().alloc_reach(composed.describe());
        let bundle = composed.without_region(dest.region());
        (Some(stored), bundle)
    }

    /// [`Self::mint`] paired with the destination-coverage bit: `true` iff some `sources` bundle
    /// pins `dest`'s own region. The bit is computed over the source bundles, so it is unaffected by
    /// the self rule's strip of the returned bundle — the multi-source generalization of the
    /// `borrows_into_dest` companion [`Carrier::mint_into`](super::Carrier::mint_into) computes for
    /// a single carrier.
    ///
    /// It duplicates what the stored description now records directly (home is an ordinary member),
    /// and is carried for call sites that read the bit off a carrier rather than querying
    /// membership. Retired with those call sites.
    pub fn mint_with_dest_bit<'a, W>(
        dest: RegionHandle<'a, W>,
        sources: &[&PinBundle<F>],
    ) -> (Option<&'a ReachDescription<F>>, PinBundle<F>, bool)
    where
        W: StorageProfile<FrameOwner = F>,
        F: RegionOwner<Region = Region<W>>,
    {
        let borrows_into_dest = sources.iter().any(|s| s.pins_region(dest.region()));
        let (stored, bundle) = Self::mint(dest, sources);
        (stored, bundle, borrows_into_dest)
    }
}

/// The owned reach witness: the antichain of `Rc<F>` owners whose regions a value's borrows reach.
/// A singleton for a single-region value (a scope, a same-region value, a producer frame) — the
/// common case — and larger for a multi-region value. Holding it pins every member region; the
/// empty bundle pins nothing (a frameless / run-region terminal is backed by a region that outlives
/// the carrier, so no held pin is required) and allocates nothing.
///
/// A holder owns its bundle and releases it by ordinary `Drop`: a binding entry drops it at entry
/// death (scope/region death, evacuation), the delivery envelope carries it across the parked
/// period, the run loop's step pin holds it for the step. Composition ([`Self::union`]) is set union
/// with outer-chain subsumption: a member is dropped when another member's [`PinsRegion::pins_region`]
/// chain already keeps its region alive, so the bundle stays an antichain of the deepest owners.
pub struct PinBundle<F: PinsRegion> {
    members: Vec<Rc<F>>,
}

impl<F: PinsRegion> PinBundle<F> {
    /// The empty bundle — a frameless / run-region terminal that needs no held pin.
    pub fn empty() -> Self {
        PinBundle {
            members: Vec::new(),
        }
    }

    /// A single region owner — the common case (a scope, a same-region value, a producer frame).
    pub fn singleton(owner: Rc<F>) -> Self {
        PinBundle {
            members: vec![owner],
        }
    }

    /// Whether this bundle holds no region owner (the frameless / run-region terminal).
    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    /// Whether any member's owner chain keeps `region`'s storage alive — the set-level lift of
    /// [`PinsRegion::pins_region`] over the bundle's **strong** members. The destination-coverage
    /// query [`ReachDescription::mint_with_dest_bit`] runs over the source bundles, before the self
    /// rule strips `dest` from the minted bundle: no `Weak` upgrade, since the bundle already owns
    /// its members.
    pub fn pins_region(&self, region: &F::Region) -> bool {
        self.members.iter().any(|m| m.pins_region(region))
    }

    /// Whether any member's own region satisfies `pred` — the bundle-side twin of
    /// [`ReachDescription::any_member_region`], for a holder with no single named target region to
    /// test (an address-table membership check against a raw stored pointer: "which member region
    /// hosts this value?"). This is how a holder locates a value's **home** now that home is an
    /// ordinary member with no distinguished field. No member reference escapes: `pred` runs
    /// against each member's region internally, so the bundle stays unenumerable.
    pub fn any_member_region(&self, mut pred: impl FnMut(&F::Region) -> bool) -> bool {
        self.members.iter().any(|m| pred(m.region()))
    }

    /// Insert `owner` under outer-chain subsumption: skip it when an existing member already pins
    /// its region (dedup + the newcomer-is-an-ancestor case), else drop every existing member the
    /// newcomer subsumes and add it. Keeps the bundle an antichain of the deepest owners.
    ///
    /// The in-place counterpart of [`Self::union`], for a holder that folds a lifted set into a
    /// long-lived bundle it already owns (a scope's union bundle at each bind) rather than
    /// rebuilding through a fresh allocation per fold.
    pub fn insert(&mut self, owner: Rc<F>) {
        if self.members.iter().any(|m| m.pins_region(owner.region())) {
            return;
        }
        self.members.retain(|m| !owner.pins_region(m.region()));
        self.members.push(owner);
    }

    /// Fold every member of `other` in through [`Self::insert`], consuming it — the whole-bundle
    /// in-place counterpart of [`Self::union`], as [`Self::insert`] is for a single owner. Consuming
    /// `other` moves its `Rc`s rather than cloning them, so a retention that hands its pins over
    /// adds no refcount traffic. This is how a long-lived holder accumulates one deduped bundle
    /// across many folds (a region's [`retain_reach`](Region::retain_reach) list, collapsed to a
    /// single antichain of the deepest owners) instead of keeping a bundle per fold.
    pub fn absorb(&mut self, other: Self) {
        for owner in other.members {
            self.insert(owner);
        }
    }

    /// The set union of `left` and `right` under outer-chain subsumption — the owner-side compose
    /// the run loop's step pin and the envelope's liveness bundle fold through.
    pub fn union(left: &Self, right: &Self) -> Self {
        let mut result = left.clone();
        for owner in &right.members {
            result.insert(Rc::clone(owner));
        }
        result
    }

    /// This bundle without any member whose region **is** `region` — the self rule
    /// (design/witness-hosting.md § Composition), applied where a bundle is about to be owned by
    /// `region` itself: a region holding a pin on its own owner is a reference cycle that frees
    /// neither. Exact-region only, by pointer identity: an *ancestor* of `region` stays, since
    /// owning a pin on an outer frame closes no cycle.
    pub fn without_region(&self, region: &F::Region) -> Self {
        PinBundle {
            members: self
                .members
                .iter()
                .filter(|m| !std::ptr::eq(m.region() as *const _, region as *const _))
                .map(Rc::clone)
                .collect(),
        }
    }

    /// This bundle keeping only the members whose region satisfies `keep` — the **retention
    /// predicate**'s filter (design/witness-hosting.md § Escape). A relocation verb derives a source
    /// claim by running the embedder's `still_borrows` over the product against each member region
    /// in turn, so a claim is a checked property of the folded bytes rather than a bundle assembled
    /// by hand. No member reference escapes: `keep` sees each member's region, never its owner.
    pub fn retaining(&self, mut keep: impl FnMut(&F::Region) -> bool) -> Self {
        PinBundle {
            members: self
                .members
                .iter()
                .filter(|m| keep(m.region()))
                .map(Rc::clone)
                .collect(),
        }
    }

    /// The description mirror of this bundle's antichain — `Weak` members for side-table hosting.
    /// Called at a mint, where the bundle is the freshly-composed antichain and the description it
    /// yields is stored frozen alongside it.
    fn describe(&self) -> ReachDescription<F> {
        ReachDescription::from_members(&self.members)
    }

    /// The bundle's members — white-box reach introspection, gated behind `test-hooks` for an
    /// embedder's white-box tests (mirroring `Scheduler::anchor_of`'s gate).
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn members(&self) -> &[Rc<F>] {
        &self.members
    }
}

impl<F: PinsRegion> Default for PinBundle<F> {
    fn default() -> Self {
        Self::empty()
    }
}

impl<F: PinsRegion> Clone for PinBundle<F> {
    fn clone(&self) -> Self {
        PinBundle {
            members: self.members.clone(),
        }
    }
}

// SAFETY: each member `Rc<F>` keeps its region's storage at a fixed heap address for the whole life
// of the `Rc` (`Rc` is `StableDeref`), so holding the bundle pins every member region. The empty
// bundle carries no pin: a frameless value is backed by storage that outlives the carrier, so no
// held pin is required.
unsafe impl<F: PinsRegion> Witness for PinBundle<F> {}

// SAFETY: `union` returns the set union (deduplicated by region, a member dropped only when another
// member's owner chain already pins its region), so holding the result keeps every region either
// input pinned alive, regardless of `dest`: an owned bundle can always represent the union, so there
// is nothing a destination allocation capability would let this impl do that plain union doesn't.
unsafe impl<F: PinsRegion, B: Reattachable> ComposeWitness<B> for PinBundle<F> {
    fn compose<'b>(left: &Self, right: &Self, _dest: &B::At<'b>) -> Self {
        Self::union(left, right)
    }
}
