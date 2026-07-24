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
//! at a [`ReachDescription::mint`] — the description into the destination's side table, the bundle
//! to the caller — or the bundle is recovered from a description under a covering witness
//! ([`ReachDescription::to_bundle`], the sole description→bundle door). No constructor builds
//! either from loose parts.

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

    /// Recover an owned [`PinBundle`] from this description — the sole description→bundle door. Every
    /// member upgrades (the holder rule), producing the owned antichain the description mirrors. The
    /// caller reads the description under coverage (the delivery envelope derives its foreign bundle
    /// here at seal time, under the host pin its carrier is read against), so the upgrades succeed by
    /// the reached regions' ambient liveness at that point.
    pub(in crate::witnessed) fn to_bundle(&self) -> PinBundle<F> {
        let mut members = Vec::with_capacity(self.members.len());
        self.for_each_owner(|owner| members.push(Rc::clone(owner)));
        PinBundle { members }
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
    /// Composes every description in `sources` (reading their **exact** members — never "everything
    /// a region reaches") plus any `materialize_hosts` (a source's old host, materialized when
    /// foreign to `dest`), applying:
    ///
    /// 1. **Home-omission** — `dest`'s own region is never a member (the self-cycle rule),
    ///    enforced here unconditionally, *plus* whatever `omit` reports already-pinned.
    /// 2. **Borrows-host materialization** — each `Rc<F>` in `materialize_hosts` becomes a member
    ///    iff its region is foreign to `dest` (and not otherwise omitted).
    /// 3. **Outer-chain subsumption** — via [`PinsRegion`], built into [`PinBundle::insert`]: a
    ///    member kept alive by another member's owner chain is dropped.
    ///
    /// The subsumption fold runs on the upgraded strong `Rc`s (the bundle), then the bundle is
    /// downgraded into the stored description, so the description's members mirror the bundle's
    /// antichain. The returned `&'a` description is co-located in `dest`'s side table; the returned
    /// bundle is what keeps its members alive. `(None, empty bundle)` when the composed reach is
    /// empty — a region-pure value mints nothing, encoded without an allocation.
    pub fn mint<'a, W>(
        dest: RegionHandle<'a, W>,
        sources: &[&ReachDescription<F>],
        materialize_hosts: &[Rc<F>],
        omit: impl Fn(&F::Region) -> bool,
    ) -> (Option<&'a ReachDescription<F>>, PinBundle<F>)
    where
        // `W::FrameOwner = F` ties the destination's reach side table to this member type, so the
        // minted description lands in `dest`'s own [`Region::alloc_reach`] table. Binding `Region`
        // on `RegionOwner` (the trait that DECLARES it, not `PinsRegion`) avoids E0220 — a
        // supertrait's associated type is not bindable through the subtrait.
        W: StorageProfile<FrameOwner = F>,
        F: RegionOwner<Region = Region<W>>,
    {
        let dest_region: *const Region<W> = dest.region();
        // Rule 1 (self-cycle) folded together with the caller's policy predicate.
        let omit_all = |r: &Region<W>| std::ptr::eq(r as *const _, dest_region) || omit(r);

        let mut bundle = PinBundle::empty();
        for source in sources {
            source.for_each_owner(|owner| {
                if !omit_all(owner.region()) {
                    bundle.insert(Rc::clone(owner)); // exact members + subsumption + omission
                }
            });
        }
        for host in materialize_hosts {
            if !omit_all(host.region()) {
                bundle.insert(Rc::clone(host)); // rule 2 + subsumption
            }
        }
        if bundle.is_empty() {
            (None, bundle)
        } else {
            // Freeze the antichain's mirror into the region's side table; the owned bundle rides out
            // to the caller (the binding entry, the delivery envelope, the region's own retention).
            let stored = dest.region().alloc_reach(bundle.describe());
            (Some(stored), bundle)
        }
    }

    /// [`Self::mint`] paired with the pre-omission destination-coverage bit: `true` iff some
    /// `sources` description or `materialize_hosts` owner pins `dest`'s own region *before*
    /// home-omission drops it. Home-omission (rule 1) removes `dest`'s region from the stored
    /// members, so the bit is the only surviving record that the value's borrows reach the
    /// destination — the multi-source generalization of the `borrows_into_dest` companion
    /// [`Carrier::mint_into`](super::Carrier::mint_into) computes for a single carrier.
    pub fn mint_with_dest_bit<'a, W>(
        dest: RegionHandle<'a, W>,
        sources: &[&ReachDescription<F>],
        materialize_hosts: &[Rc<F>],
        omit: impl Fn(&F::Region) -> bool,
    ) -> (Option<&'a ReachDescription<F>>, PinBundle<F>, bool)
    where
        W: StorageProfile<FrameOwner = F>,
        F: RegionOwner<Region = Region<W>>,
    {
        let borrows_into_dest = sources.iter().any(|s| s.pins_region(dest.region()))
            || materialize_hosts
                .iter()
                .any(|h| h.pins_region(dest.region()));
        let (stored, bundle) = Self::mint(dest, sources, materialize_hosts, omit);
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

    /// Insert `owner` under outer-chain subsumption: skip it when an existing member already pins
    /// its region (dedup + the newcomer-is-an-ancestor case), else drop every existing member the
    /// newcomer subsumes and add it. Keeps the bundle an antichain of the deepest owners.
    fn insert(&mut self, owner: Rc<F>) {
        if self.members.iter().any(|m| m.pins_region(owner.region())) {
            return;
        }
        self.members.retain(|m| !owner.pins_region(m.region()));
        self.members.push(owner);
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
