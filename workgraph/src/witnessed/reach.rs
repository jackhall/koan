//! The reach-evidence types, split by ownership. A value's *reach* — the set of foreign region
//! owners its borrows keep alive — is carried by two cooperating types over one member trait
//! ([`PinsRegion`], the outer-chain subsumption hook a workload's frame-owner type supplies):
//!
//! - [`ReachDescription<F>`] — the **non-owning** description: a `Weak<F>` host plus `Weak<F>`
//!   members, interned in a region's append-stable side table ([`Region::intern_reach_retained`]) and
//!   *referenced* (never owned) by a [`Carrier`](super::Carrier). It keeps nothing alive; it answers
//!   membership queries ([`Self::pins_region`]) and residence queries
//!   ([`Self::with_home_region`]) by upgrading under a pinned
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
//! together at a [`ReachDescription::mint_resident`] — the description into the destination's side
//! table, the owned bundle into that same region's retention — from the source **bundles** the
//! composition folds (strong `Rc` members, never a description's `Weak`). A description is never
//! upgraded to build an owned bundle: ownership flows forward from a mint, into the destination's
//! own retention and, for a value that travels on, a threaded transit copy
//! ([`ReachDescription::mint_resident_threaded`]). No constructor builds either type from loose
//! parts.
//!
//! **A mint IS a retention.** Every mint is a resident mint: the value it describes lives in the
//! destination, so the destination is what owns the pins keeping its reach alive, and no caller ever
//! receives an owned bundle to fold by hand. That is what makes an intern hit *proof* — an entry
//! exists in a region's table only because a miss retained an identical member set there — so the
//! table needs no side record of which entries the region already pins.
//!
//! A description records **two facts, not one**: its `host` is where the value *lives* (the owner of
//! the region the description is hosted in, stamped from `dest` at the mint); its `members` are the
//! regions the value's borrows *reach*. Home appears among the members only when the value genuinely
//! borrows into its own region, so membership stays exact and residence is answerable without one.
//!
//! The description a mint stores and the bundle it retains differ in exactly one member, the **self
//! rule** (design/reach.md § Composition): the description keeps every composed member,
//! `dest`'s own region included, so membership is exact; the retained bundle drops any member whose
//! region *is* `dest`'s, because a region owning a pin on itself is a reference cycle. The asymmetry
//! is what makes the `Sealed → Delivered` lift correct for a value resting in its own scope's region
//! — [`ReachDescription::to_bundle`] upgrades a home *member* along with everything else when the
//! value travels out.
//!
//! The self rule bounds a *mint*. A second rule, the **eternal rule**
//! ([`PinBundle::without_eternal`]), bounds a *region-lifetime retention*
//! ([`Region::retain_reach`]): a member declaring [`PinsRegion::needs_no_pin`] — storage that
//! already outlives every region — is dropped there. Between them they are what keeps the retention
//! graph acyclic. Neither one alone suffices: an owner outside `dest`'s own region can still hold a
//! chain back to it, and the two-region ring that closes when a run-root region retains a per-call
//! owner while that owner's region retains the run root is exactly what the eternal rule cuts.

use std::rc::{Rc, Weak};

use smallvec::{smallvec, SmallVec};

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

    /// Whether this owner's storage outlives every region that could retain it, so an owning pin on
    /// it buys nothing and taking one only risks a cycle. `false` by default — the safe answer, an
    /// extra pin never dangles. A workload overrides it for its eternal tier
    /// ([`RegionHost::is_eternal`](super::RegionHost::is_eternal)).
    ///
    /// # Safety
    ///
    /// Returning `true` asserts that `Self`'s storage — and every region its owner chain pins —
    /// stays live and fixed-address for at least as long as any region that could retain it. A
    /// lying answer drops the one pin holding a region alive, which is the dangle the whole reach
    /// system exists to rule out.
    fn needs_no_pin(&self) -> bool {
        false
    }
}

/// The non-owning reach description: a `Weak<F>` **host** naming the owner of the region the
/// description lives in — the value's residence — and the `Weak<F>` **members** naming the regions
/// the value's borrows reach. Hosted in the value's home region's own side table and referenced by
/// the carrier. The member set is a singleton for a single-region value (a lifted closure over one
/// source region), larger for a multi-region value, and empty for a region-pure value, which still
/// gets a description because that is where its residence is recorded. Holding a description pins
/// **nothing**: host and members are all `Weak`, and the owning [`PinBundle`] the holder keeps is
/// what keeps them alive.
///
/// Queries ([`Self::pins_region`] / [`Self::any_member_region`] / [`Self::with_home_region`]) run
/// under a pinned read: they upgrade, sound because whoever can reach this description holds the
/// coverage that keeps the `Rc`s alive (the holder rule). A failed upgrade is a bug — a read with no
/// covering pin — so it `debug_assert`s and, in release, is treated as naming nothing. The host is
/// strictly safer than the members: the description lives *inside* its host region's side table, so
/// reaching it at all implies the host storage is alive.
pub struct ReachDescription<F: PinsRegion> {
    /// The owner of the region this description is hosted in — the value's residence.
    host: Weak<F>,
    /// The regions the value's borrows reach. Home appears here only when the borrows reach it.
    members: ReachSet<Weak<F>>,
}

/// The storage a reach member set takes, in both the owned ([`PinBundle`]) and the described
/// ([`ReachDescription`]) form: **inline up to two members, heap past that**.
///
/// Two slots is where the shapes are. A reach is empty for a region-pure value and a singleton for a
/// single-region one — a lifted closure over one source region, a value borrowing its producer
/// frame, a step's own anchor — and those two shapes dominate every table and every fold. The pair
/// is the next commonest: a product composed from two operands living in different regions. Past
/// that a reach is a genuine multi-region antichain and pays for a heap buffer, as it did before.
///
/// The inline slots are free at this width: under smallvec's `union` representation (enabled in this
/// crate's manifest for exactly this reason) `SmallVec<[T; 2]>` measures 24 bytes for a
/// pointer-sized member — what the `Vec<T>` it replaced measured — so the common cases lose their
/// allocation without the uncommon ones growing the type. The default enum representation would
/// spend a discriminant word and cost 32.
///
/// This shrinks an allocation *count*; it moves nothing. A description stays heap-owned and
/// `Drop`-bearing whatever its arity — its members are `Weak`, whose counts must be released at
/// region death — which is what keeps it out of the region's bump ([`Region::intern_reach_retained`]).
type ReachSet<T> = SmallVec<[T; 2]>;

impl<F: PinsRegion> ReachDescription<F> {
    /// The description mirror of an antichain of owners — `Weak` members for side-table hosting,
    /// under the `host` the mint stamps from its destination. The sole builder; a description's
    /// members always mirror some [`PinBundle`]'s antichain.
    fn from_members(host: Weak<F>, members: &[Rc<F>]) -> Self {
        ReachDescription {
            host,
            members: members.iter().map(Rc::downgrade).collect(),
        }
    }

    /// Whether the value's borrows reach no region at all — a region-pure value, which still names
    /// its residence through [`Self::with_home_region`].
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

    /// The host owner, upgraded under the holder rule — the value's residence. Infallible wherever
    /// the description is reachable: it lives in the host region's own side table, so a read that
    /// can see it is covered by a pin that keeps the host storage alive.
    pub(in crate::witnessed) fn host_owner(&self) -> Rc<F> {
        self.host.upgrade().expect(
            "a reach description is hosted inside its host's own region: reaching it at \
                     all means a pin keeps that storage alive",
        )
    }

    /// Run `f` against the region the value **lives in** — its residence, stamped at the mint from
    /// the destination the description was frozen into. This is the region a workload's escape-seam
    /// policy prices a crossing *out of*, and the one member a relocation's retention predicate can
    /// release. No owner reference escapes: the upgraded host is dropped at the end of the call.
    pub fn with_home_region<R>(&self, f: impl FnOnce(&F::Region) -> R) -> R {
        f(RegionOwner::region(&*self.host_owner()))
    }

    /// Whether the value's borrows reach into its **own** home region — `members` ∋ `host`, answered
    /// as the same coverage query [`Self::pins_region`] answers for any other region. False for a
    /// region-pure value resident in that region: it lives there but borrows nothing there.
    pub fn borrows_home(&self) -> bool {
        self.with_home_region(|home| self.pins_region(home))
    }

    /// Whether any member's storage is **not** already eternal ([`PinsRegion::needs_no_pin`]) — "does
    /// this value reach a region that can die?". The description-side companion of the retention's
    /// eternal rule ([`PinBundle::without_eternal`]): a value whose whole reach is eternal storage
    /// needs no relocation to outlive a per-call frame, because nothing it names dies with one.
    pub fn pins_beyond_eternal(&self) -> bool {
        let mut hit = false;
        self.for_each_owner(|owner| {
            if !hit && !owner.needs_no_pin() {
                hit = true;
            }
        });
        hit
    }

    /// Whether any member's owner chain keeps `region`'s storage alive — the set-level lift of
    /// [`PinsRegion::pins_region`], run under a pinned read. The reach-covers query a carrier layers
    /// its finalize gate and its [`Self::borrows_home`] answer on: "does this reach already name the
    /// region I'm about to fold?".
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
    /// for a caller with no single named target region to test, only a property each candidate
    /// region is asked about in turn. No member reference escapes: `pred` runs against each upgraded
    /// member's region internally.
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

    /// Compose `sources` into one antichain — the shared front half of both mint doors. Folds each
    /// source bundle's **exact** strong members (never "everything a region reaches", never a
    /// description's `Weak`) through [`PinBundle::insert`], so outer-chain subsumption normalizes
    /// the result to the deepest owners.
    fn compose_sources(sources: &[&PinBundle<F>]) -> PinBundle<F> {
        let mut composed = PinBundle::empty();
        for source in sources {
            for owner in &source.members {
                composed.insert(Rc::clone(owner)); // exact members + subsumption
            }
        }
        composed
    }

    /// **The resident mint**: freeze the composed description into `dest`'s side table and establish
    /// its retention there in the same act (design/reach.md § Composition). Returns the
    /// description alone — the value rests in `dest`, so `dest`'s region owns the pins that keep its
    /// reach alive and no caller ever holds them.
    ///
    /// The description's `host` is stamped from `dest`'s own owner, so the value's residence is
    /// recorded by the same act that records its reach — `dest` **is** the home, since a description
    /// always lands in its value's home region's side table.
    ///
    /// Every value gets a description: a region-pure value mints one with empty members, because
    /// there is nowhere else to put the host. The empty member set is a key like any other, so
    /// every region-pure value in a region shares that region's one empty description.
    ///
    /// Composes every source bundle in `sources` — its **exact** strong members, never "everything a
    /// region reaches" — applying exactly two rules, and no caller-supplied policy:
    ///
    /// 1. **Outer-chain subsumption** — via [`PinsRegion`], built into [`PinBundle::insert`]: a
    ///    member kept alive by another member's owner chain is dropped.
    /// 2. **The self rule**, applied to the **retention only**: a member whose region *is* `dest`'s
    ///    is dropped from the bundle the region retains (a region owning a pin on itself is a cycle)
    ///    while staying a member of the stored description, so membership remains exact. Every other
    ///    member survives in both; a mint is not where eternal storage is filtered out — that is the
    ///    retention's own eternal rule ([`PinBundle::without_eternal`]), applied at
    ///    [`Region::retain_reach`], because only a region-lifetime pin can close a ring.
    ///
    /// A minted description is therefore **exact**: it is the value's whole reach, not a reach
    /// narrowed against what some destination's container happened to pin, so every consumer reads
    /// the answer off it directly instead of re-deriving what a narrowing dropped.
    ///
    /// The fold runs over the sources' **strong** `Rc` members (no `Weak` upgrade — ownership flows
    /// forward from the mint, never recovered from a description), then the composed antichain is
    /// downgraded into the stored description, so the description's members mirror it.
    ///
    /// The composed antichain is **interned with its retention**
    /// ([`Region::intern_reach_retained`]): a member set already described in `dest` yields that
    /// existing entry and folds nothing, because the entry's own miss already folded an identical
    /// bundle into the region's union. One description exists per distinct reach per region, pointer
    /// identity over descriptions *is* member-set equality within a region, and an entry's existence
    /// *is* the proof that `dest` pins what it names.
    pub(crate) fn mint_resident<'a, W>(
        dest: RegionHandle<'a, W>,
        sources: &[&PinBundle<F>],
    ) -> &'a ReachDescription<F>
    where
        // `W::FrameOwner = F` ties the destination's reach side table to this member type, so the
        // minted description lands in `dest`'s own [`Region::intern_reach_retained`] table. Binding
        // `Region` on `RegionOwner` (the trait that DECLARES it, not `PinsRegion`) avoids E0220 — a
        // supertrait's associated type is not bindable through the subtrait.
        W: StorageProfile<FrameOwner = F>,
        F: RegionOwner<Region = Region<W>>,
    {
        dest.region()
            .intern_reach_retained(Self::compose_sources(sources))
    }

    /// [`Self::mint_resident`] additionally handing back the self-rule-stripped composed bundle —
    /// for the composition engine alone, whose product travels on inside a delivery envelope and so
    /// needs **transit pins of its own** on top of the destination's own region-lifetime retention.
    /// The retention is the resident mint's verbatim; the extra bundle duplicates it rather than
    /// replacing it, so the two liveness tiers are independent.
    ///
    /// Crate-internal and deliberately narrow: handing the pins out is what a resident mint exists
    /// to avoid, so the only caller is the one that must re-own them to travel.
    pub(in crate::witnessed) fn mint_resident_threaded<'a, W>(
        dest: RegionHandle<'a, W>,
        sources: &[&PinBundle<F>],
    ) -> (&'a ReachDescription<F>, PinBundle<F>)
    where
        W: StorageProfile<FrameOwner = F>,
        F: RegionOwner<Region = Region<W>>,
    {
        let composed = Self::compose_sources(sources);
        // The transit copy carries the same members the region retains — the self rule applies to
        // both, since a bundle naming `dest`'s own region would keep that region alive through a
        // value living inside it.
        let mut travelling = composed.clone();
        travelling.remove_region(dest.region());
        let description = dest.region().intern_reach_retained(composed);
        (description, travelling)
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
    members: ReachSet<Rc<F>>,
}

impl<F: PinsRegion> PinBundle<F> {
    /// The empty bundle — a frameless / run-region terminal that needs no held pin.
    pub fn empty() -> Self {
        PinBundle {
            members: ReachSet::new(),
        }
    }

    /// A single region owner — the common case (a scope, a same-region value, a producer frame).
    pub fn singleton(owner: Rc<F>) -> Self {
        PinBundle {
            members: smallvec![owner],
        }
    }

    /// Whether this bundle holds no region owner (the frameless / run-region terminal).
    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    /// Whether any member's owner chain keeps `region`'s storage alive — the set-level lift of
    /// [`PinsRegion::pins_region`] over the bundle's **strong** members. No `Weak` upgrade, since
    /// the bundle already owns its members.
    pub fn pins_region(&self, region: &F::Region) -> bool {
        self.members.iter().any(|m| m.pins_region(region))
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
    /// (design/reach.md § Composition), applied where a bundle is about to be owned by
    /// `region` itself: a region holding a pin on its own owner is a reference cycle that frees
    /// neither. Exact-region only, by pointer identity: an *ancestor* of `region` stays, since
    /// owning a pin on an outer frame closes no cycle.
    pub fn without_region(&self, region: &F::Region) -> Self {
        let mut without = self.clone();
        without.remove_region(region);
        without
    }

    /// [`Self::without_region`] applied **in place**, for a caller that already owns the bundle it
    /// is about to hand on — the composed antichain at a [`ReachDescription::mint_resident`], an
    /// operand's pins moved out of a consumed envelope. Dropping the self member is a `retain` over
    /// storage the caller has already paid for, so the self rule costs no second buffer and no
    /// refcount traffic: the surviving members are moved, never cloned and re-dropped.
    pub(in crate::witnessed) fn remove_region(&mut self, region: &F::Region) {
        self.members
            .retain(|m| !std::ptr::eq(m.region() as *const _, region as *const _));
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

    /// This bundle without any member whose storage already outlives every region
    /// ([`PinsRegion::needs_no_pin`]) — the **eternal rule**, the region-lifetime companion to the
    /// self rule. Applied where a bundle is about to be owned *for a region's whole life*
    /// ([`Region::retain_reach`]): pinning storage that outlives the region buys nothing, and if
    /// that storage's own region ever retains this one back, the pair pins each other forever.
    pub(crate) fn without_eternal(&self) -> Self {
        PinBundle {
            members: self
                .members
                .iter()
                .filter(|m| !m.needs_no_pin())
                .map(Rc::clone)
                .collect(),
        }
    }

    /// The description mirror of this bundle's antichain — `Weak` members for side-table hosting,
    /// under `host` (the owner of the region the description is frozen into). Called on an intern
    /// miss, where the bundle is the freshly-composed antichain and the description it yields is
    /// stored frozen alongside it.
    pub(in crate::witnessed) fn describe(&self, host: Weak<F>) -> ReachDescription<F> {
        ReachDescription::from_members(host, &self.members)
    }

    /// This bundle's canonical intern key: its member owners' addresses, **sorted**. The key
    /// [`Region::intern_reach_retained`] keys its side table on, well-defined on two grounds.
    ///
    /// The antichain is unique *as a set*: [`Self::insert`]'s outer-chain subsumption normalizes to
    /// the deepest owners regardless of fold order, so only the stored order varies and sorting
    /// removes it. And the addresses are stable for as long as an entry keyed on them lives: the
    /// entry's `Weak` members keep each `Rc<F>` allocation itself alive through the weak count, so
    /// no member address is reused while a description naming it exists.
    ///
    /// The host never enters the key — every description in one table shares that table's region
    /// owner as host.
    ///
    /// Returned in the same inline-up-to-two storage the members use ([`ReachSet`]), because a key
    /// is built on **every** mint and kept only by the one that misses. A hot path's reach shape is
    /// invariant across iterations — reach is a property of the regions involved, not the values —
    /// so the table's steady state is all hits, and a probe that allocated would allocate and free a
    /// key per mint to rediscover an entry the table already holds. [`Region::intern_reach_retained`] boxes
    /// the key at the insert, where the map keeps it.
    pub(in crate::witnessed) fn intern_key(&self) -> ReachSet<usize> {
        let mut key: ReachSet<usize> = self
            .members
            .iter()
            .map(|m| Rc::as_ptr(m) as usize)
            .collect();
        key.sort_unstable();
        key
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

/// The **embedder-facing** owned-coverage holder: a [`PinBundle`] an embedder may hold, clone,
/// thread and drop — but not compute with. It is the "step's coverage" of
/// design/reach.md § Threading, and the shape every owned pin crosses the library
/// boundary in: a step carries one from the fold that composed it to the seal that consumes it, a
/// finalize hands one to the retention hold, a region retains one for its life.
///
/// The point is what it *lacks*. `PinBundle`'s arithmetic — [`union`](PinBundle::union),
/// [`without_region`](PinBundle::without_region), [`retaining`](PinBundle::retaining),
/// [`insert`](PinBundle::insert), [`absorb`](PinBundle::absorb) — is crate-private, so an embedder
/// cannot narrow a claim, strip a member, or assemble a bundle by hand. Every set operation is a
/// container verb on the holder that owns the pins, which is what makes the pinning invariant
/// something the embedder has no vocabulary to break rather than a rule it is asked to honor.
///
/// It is a [`Witness`]: holding one keeps every member region live, so a read may run under it.
pub struct StepCoverage<F: PinsRegion>(pub(crate) PinBundle<F>);

impl<F: PinsRegion> StepCoverage<F> {
    /// Coverage of nothing — the frameless / run-region terminal, and the seed a fold accumulates
    /// from. Allocates nothing.
    pub fn empty() -> Self {
        StepCoverage(PinBundle::empty())
    }

    /// Coverage of one region, from an owner the caller already holds (a step's anchor frame, a
    /// scope's own region owner). Handing in an `Rc` is not pin arithmetic: the caller already has
    /// the pin, and this only names it as coverage.
    pub fn of(owner: Rc<F>) -> Self {
        StepCoverage(PinBundle::singleton(owner))
    }

    /// Widen this coverage to also cover everything `other` covers — the union a step assembles
    /// across its deps. Consuming, so a hand-off clones no `Rc`.
    pub fn absorb(&mut self, other: StepCoverage<F>) {
        self.0.absorb(other.0);
    }

    /// Whether this coverage holds no region owner.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl<F: PinsRegion> Clone for StepCoverage<F> {
    fn clone(&self) -> Self {
        StepCoverage(self.0.clone())
    }
}

impl<F: PinsRegion> Default for StepCoverage<F> {
    fn default() -> Self {
        Self::empty()
    }
}

// SAFETY: the wrapped `PinBundle` is a `Witness` — holding it keeps every member region's storage
// live at a fixed address — and this newtype adds nothing but the narrowed surface.
unsafe impl<F: PinsRegion> Witness for StepCoverage<F> {}
