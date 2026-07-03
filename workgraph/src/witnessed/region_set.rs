//! The opaque reach-set library type: [`RegionSet<F>`], generic over a member trait
//! ([`PinsRegion`]) that supplies the outer-chain subsumption hook a workload's frame-owner type
//! implements. Mechanism (subsumption, folding, union) is library-owned; member semantics (what
//! "pins" means for a workload's frame type) is workload-supplied through the trait.

use std::rc::Rc;

use super::{MergeWitness, RegionOwner, SetWitness, Witness};

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

    /// The set union of `left` and `right` under outer-chain subsumption.
    pub fn union(left: &Self, right: &Self) -> Self {
        let mut result = left.clone();
        for owner in &right.members {
            result.insert(Rc::clone(owner));
        }
        result
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

// SAFETY: `merge` returns the set union (deduplicated by region, a member dropped only when
// another member's owner chain already pins its region), so holding the result keeps every region
// either input pinned alive. Always `Some` — a set can always represent the union.
unsafe impl<F: PinsRegion> MergeWitness for RegionSet<F> {
    fn merge(left: &Self, right: &Self) -> Option<Self> {
        Some(Self::union(left, right))
    }
}
