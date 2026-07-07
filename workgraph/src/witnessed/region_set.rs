//! The opaque reach-set library type: [`RegionSet<F>`], generic over a member trait
//! ([`PinsRegion`]) that supplies the outer-chain subsumption hook a workload's frame-owner type
//! implements. Mechanism (subsumption, folding, union) is library-owned; member semantics (what
//! "pins" means for a workload's frame type) is workload-supplied through the trait.

use std::rc::Rc;

use super::{
    ComposeWitness, Reattachable, Region, RegionHandle, RegionOwner, SetWitness, StorageProfile,
    Stored, UnionWitness, Witness,
};

/// A [`RegionOwner`] that can report whether holding it keeps another region's storage alive — the
/// outer-chain subsumption hook [`RegionSet`] folds and inserts through.
///
/// # Safety
///
/// `pins_region(r) == true` asserts that holding `Self` (behind its `Rc`) keeps the storage of the
/// region at `r` live and at a fixed address for as long as `Self` is held — `Self`'s own region or
/// one reached through an owner chain it pins. This is what makes subsumption sound: `RegionSet`
/// drops a member whose region another member already pins, and the remaining member must
/// genuinely carry that pin.
pub unsafe trait PinsRegion: RegionOwner {
    /// Whether holding `self` keeps the storage of `region` alive.
    fn pins_region(&self, region: &Self::Region) -> bool;
}

/// The unified region-owner witness: the set of `Rc<F>` whose regions a carrier's value reaches. A
/// singleton for a single-region value (a scope, a same-region value, a producer frame) — the
/// common case — and larger for a multi-region value (a lifted closure reaching several source
/// regions). Holding it pins every member region; the empty set pins nothing — a frameless /
/// run-region terminal is backed by a region that outlives the carrier, so no held pin is required.
///
/// Composition ([`Self::union`]) is set union with outer-chain subsumption: a member is dropped
/// when another member's [`PinsRegion::pins_region`] chain already keeps its region alive, so the
/// set stays an antichain of the deepest owners (a singleton whenever the members are co-lineal).
///
/// Backed by a `Vec` (a singleton in the common case); the inline `SmallVec` representation is an
/// open optimization owned elsewhere.
pub struct RegionSet<F: PinsRegion> {
    members: Vec<Rc<F>>,
}

impl<F: PinsRegion> RegionSet<F> {
    /// The empty witness — a frameless / run-region terminal that needs no held pin.
    pub fn empty() -> Self {
        RegionSet {
            members: Vec::new(),
        }
    }

    /// A single region owner — the common case (a scope, a same-region value, a producer frame).
    pub fn singleton(owner: Rc<F>) -> Self {
        RegionSet {
            members: vec![owner],
        }
    }

    /// Whether this set holds no region owner (the frameless / run-region terminal).
    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    /// The sole region owner of a singleton set, or `None` for empty / multi-member sets — the hook
    /// the consumer-pull lift uses to recover the producer owner from a finalized terminal's
    /// witness (a finalized value is produced in exactly one frame, so its witness is a singleton).
    pub fn sole(&self) -> Option<&Rc<F>> {
        match self.members.as_slice() {
            [only] => Some(only),
            _ => None,
        }
    }

    /// The set's members — the pinned read a hosted set exposes. Read-only: a stored set is
    /// reached only by shared `&`, so this is the whole mutation-free surface over its members.
    pub fn members(&self) -> &[Rc<F>] {
        &self.members
    }

    /// Insert `owner` under outer-chain subsumption: skip it when an existing member already pins
    /// its region (dedup + the newcomer-is-an-ancestor case), else drop every existing member the
    /// newcomer subsumes and add it. Keeps the set an antichain of the deepest owners.
    fn insert(&mut self, owner: Rc<F>) {
        if self.members.iter().any(|m| m.pins_region(owner.region())) {
            return;
        }
        self.members.retain(|m| !owner.pins_region(m.region()));
        self.members.push(owner);
    }

    /// Fold every member of `other` into `self`, skipping any whose region `omit` reports as
    /// already kept alive — the predicate form the per-scope reach-set uses, which must omit
    /// regions the caller's policy considers already pinned (home frame, lexical ancestors) that
    /// [`PinsRegion::pins_region`] alone cannot see.
    pub fn fold_omitting(&mut self, other: &Self, omit: impl Fn(&F::Region) -> bool) {
        for owner in &other.members {
            if omit(owner.region()) {
                continue;
            }
            self.insert(Rc::clone(owner));
        }
    }

    /// Whether any member's owner chain keeps `region`'s storage alive — the set-level lift of
    /// [`PinsRegion::pins_region`]. The reach-covers query a carrier-witness type layers its finalize
    /// gate and bind-bit derivation on: "does this reach already name the region I'm about to fold?".
    pub fn pins_region(&self, region: &F::Region) -> bool {
        self.members.iter().any(|m| m.pins_region(region))
    }

    /// The set union of `left` and `right` under outer-chain subsumption.
    pub fn union(left: &Self, right: &Self) -> Self {
        let mut result = left.clone();
        for owner in &right.members {
            result.insert(Rc::clone(owner));
        }
        result
    }
}

// SAFETY: `RegionSet<F>` has no lifetime parameter — `At<'r>` is the same type for every `'r`, so
// it is trivially layout-invariant (the `Reattachable` contract). Mirrors the lifetime-free
// `OperatorGroup` family.
unsafe impl<F: PinsRegion + 'static> Reattachable for RegionSet<F> {
    type At<'r> = RegionSet<F>;
}

impl<F: PinsRegion + 'static> RegionSet<F> {
    /// Mint a frozen witness set into `dest`'s arena — the only way a stored set comes to exist
    /// (design/witness-hosting.md § Composition). Composes every set in `sources` (reading their
    /// **exact** member lists — never "everything a region reaches") plus any `materialize_hosts`
    /// (a source's old host, materialized when foreign to `dest`), applying:
    ///
    /// 1. **Home-omission** — `dest`'s own region is never a member (the self-cycle rule),
    ///    enforced here unconditionally, *plus* whatever `omit` reports already-pinned.
    /// 2. **Borrows-host materialization** — each `Rc<F>` in `materialize_hosts` becomes a
    ///    member iff its region is foreign to `dest` (and not otherwise omitted).
    /// 3. **Outer-chain subsumption** — via `PinsRegion`, already built into `insert` /
    ///    `fold_omitting`: a member kept alive by another member's owner chain is dropped.
    ///
    /// The result is stored frozen in `dest`'s arena; the returned `&'a` is co-located, so
    /// holding `dest`'s region owner (or this borrow) pins it and, through its members, every
    /// region it names. `None` when the composed set is empty — a region-pure value mints the
    /// empty set, encoded without an allocation.
    pub fn mint<'a, W>(
        dest: RegionHandle<'a, W>,
        sources: &[&RegionSet<F>],
        materialize_hosts: &[Rc<F>],
        omit: impl Fn(&F::Region) -> bool,
    ) -> Option<&'a RegionSet<F>>
    where
        W: StorageProfile,
        // Bind `Region` on `RegionOwner`, the trait that DECLARES it — not on `PinsRegion`.
        // `F: PinsRegion<Region = Region<W>>` is E0220 ("associated type `Region` not found for
        // `PinsRegion`"): a supertrait's associated type is not bindable through the subtrait.
        F: RegionOwner<Region = Region<W>>,
        RegionSet<F>: Stored<W> + for<'r> Reattachable<At<'r> = RegionSet<F>>,
    {
        let dest_region: *const Region<W> = dest.region();
        // Rule 1 (self-cycle) folded together with the caller's policy predicate.
        let omit_all = |r: &Region<W>| std::ptr::eq(r as *const _, dest_region) || omit(r);

        let mut composed = RegionSet::empty();
        for source in sources {
            composed.fold_omitting(source, omit_all); // exact members + subsumption + omission
        }
        for host in materialize_hosts {
            if !omit_all(host.region()) {
                composed.insert(Rc::clone(host)); // rule 2 + subsumption
            }
        }
        if composed.is_empty() {
            None
        } else {
            Some(dest.alloc_resident::<RegionSet<F>>(composed)) // freeze-at-store
        }
    }
}

impl<F: PinsRegion> Default for RegionSet<F> {
    fn default() -> Self {
        Self::empty()
    }
}

impl<F: PinsRegion> Clone for RegionSet<F> {
    fn clone(&self) -> Self {
        RegionSet {
            members: self.members.clone(),
        }
    }
}

// SAFETY: each member `Rc<F>` keeps its region's storage at a fixed heap address for the whole
// life of the `Rc` (`Rc` is `StableDeref`), so holding the set pins every member region. The empty
// set carries no pin: a frameless value is backed by storage that outlives the carrier, so no held
// pin is required.
unsafe impl<F: PinsRegion> Witness for RegionSet<F> {}

// SAFETY: `singleton(owner)` holds `owner` as the set's sole member, so the `Witness` impl above
// pins `owner`'s region for as long as the set is held — exactly the region `owner: WitnessRegion`
// pins, and no other. The single→set lift asserts nothing beyond the existing `RegionSet` witness
// fact.
unsafe impl<F: PinsRegion> SetWitness<Rc<F>> for RegionSet<F> {
    fn singleton(single: Rc<F>) -> RegionSet<F> {
        RegionSet::singleton(single)
    }
}

// SAFETY: `union` returns the set union (deduplicated by region, a member dropped only when
// another member's owner chain already pins its region), so holding the result keeps every region
// either input pinned alive.
unsafe impl<F: PinsRegion> UnionWitness for RegionSet<F> {
    fn union(left: &Self, right: &Self) -> Self {
        Self::union(left, right)
    }
}

// SAFETY: identical to the `UnionWitness` impl above — the plain union already keeps every region
// either input pinned alive, regardless of `dest`: an owned set can always represent the union, so
// there is nothing a destination allocation capability would let this impl do that plain union
// doesn't already achieve.
unsafe impl<F: PinsRegion, B: Reattachable> ComposeWitness<B> for RegionSet<F> {
    fn compose<'b>(left: &Self, right: &Self, _dest: &B::At<'b>) -> Self {
        Self::union(left, right)
    }
}
