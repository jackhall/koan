//! [`Carrier<F>`] — the reference-only carrier witness: a *reference* to a hosted reach description
//! and nothing else, the same shape whether the value is resident in a region or walking between
//! nodes. See
//! [design/reach.md § The carrier states](../../design/reach.md#the-carrier-states).
//!
//! The carrier **owns no pin**: cloning is a reference-copy, and a carrier's death releases nothing.
//! What keeps its description (and the value it describes) alive is external —
//! the container's liveness when resident, the scheduler's frame-retention hold (travelling as the
//! [`Delivered`](super::Delivered) envelope) when walking — so every re-anchor of the erased reach
//! reference happens under a named pin. The description a carrier references is never owned by it:
//! [`ReachDescription::mint`] is the only way one comes to exist, and it always
//! lands in the value's home region's own side table, so whatever covers the home region covers the
//! reference. The description is non-owning (host and members are `Weak`); the owned [`PinBundle`]
//! that pins the reached regions is held by the value's holder (a binding entry, the delivery
//! envelope), never by the carrier.
//!
//! The description carries **both** of the value's region facts, so the carrier needs no side
//! channel for either: its `host` is where the value lives (residence, read through
//! [`Opened::with_home_region`]), its members are what the value's borrows reach (read through
//! [`Opened::reach_covers`]). "Does this value borrow into its own home?" is therefore the ordinary
//! membership query [`Opened::borrows_home`], not a bit. The one asymmetry is the self rule at the
//! owned-upgrade boundary — a region never pins itself — which [`ReachDescription::mint`] applies to
//! the owned bundle alone.
//!
//! `Carrier` is deliberately **not** a [`super::Witness`]: a bare [`super::Sealed::open`] under it
//! does not compile. Reads name their coverage — [`super::Sealed::open_with`] under an external
//! pin, the envelope's [`Delivered::open`](super::Delivered::open), or the borrow-tied
//! [`Opened`](super::Opened) state, which is the only state that answers a membership query —
//! and relocations run through the envelope-bearing mint verbs
//! ([`Delivered::mint_reach`](super::Delivered::mint_reach),
//! [`Delivered::transfer_into`](super::Delivered::transfer_into)). [`Self::mint_into`] is the
//! crate-internal core they route through; it is not part of the public surface.
//!
//! A carrier is also a *source* a composition can be derived from, not only a witness a value
//! carries: [`RegionHandle::mint_retained_from_carriers`] mints one description out of several
//! resting cells' claims. That is the door for a workload whose values carry their reach rather
//! than travel with it — the composition stays here, so no embedder unions member sets of its own.

use std::rc::Rc;

use super::{
    Delivered, Erased, Opened, PinBundle, PinsRegion, ReachDescription, Reattachable, Region,
    RegionHandle, RegionOwner, StorageProfile, Witness, Witnessed,
};
// `with_branded_ref` re-anchors the erased reach reference: for the `Sealed → Delivered` lift's
// description-to-bundle upgrade ([`Carrier::upgrade_bundle`]) and for the membership queries the
// [`Opened`] state exposes.
use super::with_branded_ref;

/// [`Reattachable`] family for a lifetime-erased `&ReachDescription<F>` — the erased reach
/// reference a [`Carrier`] re-anchors under an externally supplied pin. Module-private: the pinned
/// reader on [`Carrier`] is the sole re-anchor site, so no branded reader escapes this module.
struct HostedSetRef<F>(std::marker::PhantomData<F>);

// SAFETY: `&'r ReachDescription<F>` is a thin pointer whose layout does not depend on `'r`.
unsafe impl<F: PinsRegion + 'static> Reattachable for HostedSetRef<F> {
    type At<'r> = &'r ReachDescription<F>;
}

/// A destination family's live form that exposes the [`RegionHandle`] mint target a reach
/// composition needs to allocate a hosted set into — the compose-time counterpart of
/// [`super::WitnessRegion::region`] for a region/builder family's *live* form rather than its
/// witness. Each destination family a relocation/merge site uses (a region-ref family, an
/// aggregate-builder family) implements this once for its `At<'b>`.
///
/// # Safety
///
/// The returned handle must authorize allocation into the region `dest`'s live form builds into,
/// so a set minted through it is co-located with the relocated value it describes.
pub unsafe trait HasRegionHandle<'b, P: StorageProfile> {
    /// The region handle `dest` (or the region it builds into) allocates through.
    fn region_handle(&self) -> RegionHandle<'b, P>;
}

// SAFETY: the operand IS the destination handle; the region it authorizes allocation into is
// definitionally the region a carrier composed against it re-homes into.
unsafe impl<'b, P: StorageProfile> HasRegionHandle<'b, P> for RegionHandle<'b, P> {
    fn region_handle(&self) -> RegionHandle<'b, P> {
        *self
    }
}

// SAFETY: a handle-headed operand re-homes through its head — the only 'b-branded allocation
// capability its live form carries. Peer of the (&'b Region<P>, T) blanket in step_ctx.rs; an
// embedder whose operand heads are a `RegionHandle` newtype veneer (rather than a bare
// `&'b Region<P>`) discharges its `HasRegionHandle` obligation through this blanket instead of a
// per-family impl of its own.
unsafe impl<'b, P: StorageProfile, T> HasRegionHandle<'b, P> for (RegionHandle<'b, P>, T) {
    fn region_handle(&self) -> RegionHandle<'b, P> {
        self.0
    }
}

/// The reference-only carrier witness: a reference to the reach description living in the value's
/// home region's own arena. `F` is the workload's frame-owner type (`Rc<F>` is the home pin the
/// *envelope*, not the carrier, holds). Clone is a reference-copy — no refcount traffic; the carrier
/// keeps nothing alive.
///
/// There is no empty carrier: every value has a description, because that is where its residence is
/// recorded. A region-pure value references one with empty members.
pub struct Carrier<F: PinsRegion + 'static> {
    /// The value's reach description — residence and reach in one record — erased and re-anchored
    /// only under an externally supplied pin.
    reach: Erased<HostedSetRef<F>>,
}

impl<F: PinsRegion + 'static> Clone for Carrier<F> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<F: PinsRegion + 'static> Copy for Carrier<F> {}

impl<F: PinsRegion + 'static> Carrier<F> {
    /// Reference an already-minted reach description as a carrier — the resident-read constructor:
    /// a binding entry stores the description reference this rebuilds a read carrier from. The
    /// description was minted by the library at bind time into the value's home arena; this
    /// constructor only re-packages the reference, so reach totality and residence both still rest
    /// on the mint. The carrier pins nothing — the read that re-anchors `reach` names its pin there.
    pub fn new(reach: &ReachDescription<F>) -> Self {
        Carrier {
            reach: Erased::erase(reach),
        }
    }

    /// Upgrade this carrier's reach description into an owned [`PinBundle`] under `pin` — the
    /// `Weak → Rc` upgrade the [`Sealed → Delivered`](super::Sealed) **lift** routes: a value read
    /// out of an arena-hosted seal is re-owned so the source frame may die in transit while the
    /// bundle keeps its reached regions alive. `pin` covers the description's hosting arena for the
    /// whole call (the holder rule), so [`ReachDescription::to_bundle`]'s member upgrades all
    /// succeed. A region-pure carrier (empty members) yields the empty bundle.
    pub(in crate::witnessed) fn upgrade_bundle<Pin: Witness>(&self, pin: &Pin) -> PinBundle<F> {
        // `pin` keeps the description's hosting arena live for the whole call — the same role the
        // envelope host plays for a reach read; the branded re-anchor confines the reference exactly
        // as `with_reach_impl` does, and the upgrade re-owns the members before it ends.
        let _ = pin;
        self.with_reach_impl(|reach| reach.to_bundle())
    }

    /// This carrier's **whole claim** as an owned bundle: the region the value lives in (its
    /// description's host) unioned with every region its borrows reach (the members, upgraded
    /// `Weak → Rc`), read under `pin`. It is exactly the member set a delivery envelope holds for
    /// the same value — home as an ordinary member — recovered from the description for a carrier
    /// **at rest**, whose ownership lives somewhere else (a region's union bundle) rather than in an
    /// envelope beside it.
    ///
    /// `pin` covers the description's hosting arena for the whole call (the holder rule), which is
    /// what makes the host and member upgrades succeed. Crate-internal: it hands back owned pins,
    /// the ownership tier an embedder has no vocabulary for — the composition door
    /// [`RegionHandle::mint_retained_from_carriers`] folds it and keeps it.
    pub(in crate::witnessed) fn claimed_bundle<Pin: Witness>(&self, pin: &Pin) -> PinBundle<F> {
        let members = self.upgrade_bundle(pin);
        PinBundle::union(&PinBundle::singleton(self.home_owner()), &members)
    }

    /// The owner of the region this value **lives in**, read off its description's host and upgraded
    /// under the caller's coverage. Crate-internal: it hands out an owned pin, the ownership tier an
    /// embedder has no vocabulary for — the envelope's relocation verbs use it to give their product
    /// the destination's own residence.
    pub(in crate::witnessed) fn home_owner(&self) -> Rc<F> {
        self.with_reach_impl(|reach| reach.host_owner())
    }

    /// Read the reach description this carrier references, re-anchored for the call. The
    /// crate-internal core behind [`Opened::with_reach`] — the caller is responsible for the
    /// coverage the re-anchor needs, which is why the only public route in is through the
    /// [`Opened`] state, whose `'b` **is** the pin borrow.
    fn with_reach_impl<R>(&self, f: impl FnOnce(&ReachDescription<F>) -> R) -> R {
        with_branded_ref::<HostedSetRef<F>, R>(self.reach.as_static(), |set_ref: &&_| f(set_ref))
    }

    /// Mint this carrier's reach into `dest` — the shared core the crate-internal mint verbs route
    /// through ([`Delivered::mint_reach`](super::Delivered::mint_reach)). Not itself part of the
    /// public surface. Applies, via [`ReachDescription::mint`]: outer-chain subsumption and the self
    /// rule on the returned bundle — no caller policy, so the minted description is exact and its
    /// host is `dest`'s own owner.
    ///
    /// `source` is the holder's owned pin bundle — the delivery envelope's pins, a binding entry's
    /// pins — which names the value's home as an ordinary member alongside everything else it
    /// reaches. It is threaded in, never recovered from the carrier's description, so the
    /// composition folds strong `Rc`s. Returns the minted description hosted in `dest` and the owned
    /// [`PinBundle`] the caller keeps to pin its members.
    pub(in crate::witnessed) fn mint_into<'d, P>(
        &self,
        source: &PinBundle<F>,
        dest: RegionHandle<'d, P>,
    ) -> (&'d ReachDescription<F>, PinBundle<F>)
    where
        P: StorageProfile<FrameOwner = F> + 'static,
        F: RegionOwner<Region = Region<P>>,
    {
        ReachDescription::mint(dest, &[source])
    }

    /// The relocation composition behind the envelope's
    /// [`transfer_into`](super::Delivered::transfer_into) and the live-carrier reach merges: mint
    /// BOTH operands' exact reach — the destination's (an accumulator's prior folds, threaded as
    /// `right_bundle`) and the newly-folded source's (`left_bundle`) — into `dest`'s arena. Never
    /// the source alone, or a multi-step accumulator fold would drop everything folded before this
    /// step. Both operand bundles are owned and threaded in — the composition folds strong `Rc`s,
    /// never a description's `Weak` — and `left_bundle` is where the folded value's home enters, as
    /// an ordinary member.
    ///
    /// Neither operand *carrier* is an input: everything the product reaches arrives through the two
    /// threaded bundles, and everything it resides in is `dest`, which the mint stamps as the fresh
    /// description's host.
    ///
    /// Returns the composed carrier paired with the freshly-minted owned bundle: `dest`'s region
    /// **retains a clone** for the region's life (what keeps the relocated value's reach alive when
    /// the product is consumed in place — read directly rather than re-enveloped), and the returned
    /// bundle threads to the next fold step or the terminal seal.
    pub(in crate::witnessed) fn compose_into<'b, P>(
        left_bundle: &PinBundle<F>,
        right_bundle: &PinBundle<F>,
        dest: RegionHandle<'b, P>,
    ) -> (Self, PinBundle<F>)
    where
        P: StorageProfile<FrameOwner = F> + 'static,
        F: RegionOwner<Region = Region<P>>,
    {
        let (minted, bundle) = ReachDescription::mint(dest, &[left_bundle, right_bundle]);
        dest.region().retain_for(minted, bundle.clone());
        (Carrier::new(minted), bundle)
    }
}

impl<T: Reattachable, F: PinsRegion + 'static> Witnessed<T, Carrier<F>> {
    /// Bundle a **region-pure** value under a carrier hosted in `home`'s own region — the
    /// reference-only counterpart of [`Witnessed::resident`](super::Witnessed::resident), which a
    /// [`Carrier`] cannot use because there is no default carrier to name a residence with. Mints a
    /// description with empty members (the value's borrows reach nothing) whose host is `home`, so
    /// the value records where it lives even though it reaches nowhere. The mint composes no source,
    /// so the bundle it yields is empty and nothing needs retaining.
    ///
    /// The obligation it carries is exactly
    /// [`Witnessed::resident`](super::Witnessed::resident)'s: that `value`'s reach is genuinely
    /// empty and it genuinely lives in `home`'s region. A value that references another region takes
    /// the [`yoke`](super::Witnessed::yoke) / merge path instead.
    pub fn resident_in<P>(value: T::At<'_>, home: &Rc<F>) -> Self
    where
        P: StorageProfile<FrameOwner = F> + 'static,
        F: RegionOwner<Region = Region<P>>,
    {
        let carrier = {
            let (reach, _empty) = ReachDescription::mint(RegionHandle::from_owner(&**home), &[]);
            Carrier::new(reach)
        };
        Witnessed::from_erased(Erased::erase(value), carrier)
    }
}

/// The membership and residence queries, on the **in-use** carrier state: an [`Opened`] borrows at
/// `'b` under the pin that was presented to open it, and that borrow is exactly the coverage
/// re-anchoring the erased reach reference requires — so these need no `pin` argument, and there is
/// no way to ask the question without one (design/witness-hosting.md § The carrier states).
impl<'b, T: Reattachable, F: PinsRegion + 'static> Opened<'b, T, Carrier<F>> {
    /// Whether the value's borrows reach `region` — home included when the borrows genuinely reach
    /// it, which is the question [`Self::borrows_home`] asks against the value's own residence.
    pub fn reach_covers(&self, region: &F::Region) -> bool {
        self.with_reach(|reach| reach.pins_region(region))
    }

    /// Whether the value's borrows reach into its **own** home region — `members` ∋ `host`. False
    /// for a region-pure value resident in that region.
    pub fn borrows_home(&self) -> bool {
        self.with_reach(ReachDescription::borrows_home)
    }

    /// Whether the value's borrows reach **any** region — the empty-pins gate, read off the
    /// description's members rather than off the presence of a description (every value has one).
    pub fn has_reach_members(&self) -> bool {
        self.with_reach(|reach| !reach.is_empty())
    }

    /// Run `f` against the region the value **lives in** — its residence, stamped as the
    /// description's host at the mint that froze it into that region's side table.
    pub fn with_home_region<R>(&self, f: impl FnOnce(&F::Region) -> R) -> R {
        self.with_reach(|reach| reach.with_home_region(f))
    }

    /// Read the reach description this open's carrier references, re-anchored under the open's own
    /// `'b` pin borrow.
    pub fn with_reach<R>(&self, f: impl FnOnce(&ReachDescription<F>) -> R) -> R {
        self.witness().with_reach_impl(f)
    }

    /// **The relocation seam.** Re-seal this open and lift it into a delivery envelope owning its
    /// whole reach: the carrier's own description members upgraded `Weak → Rc`, plus the region the
    /// value lives in — read off the description's own host, so no caller pairs the value with a
    /// residence it did not derive.
    ///
    /// This is what lets a value parted from a container ([`Sectioned::project`](super::Sectioned))
    /// travel. The projection is `'b`-confined by its type and states *exactly* its own run's reach;
    /// this is the one place that reach becomes owned, and it stays exact — the container's union
    /// never enters. The upgrade runs under the `'b` pin the open borrows, which is the holder rule.
    pub fn lift_out(self) -> Delivered<T, Carrier<F>, F>
    where
        F: RegionOwner,
    {
        let home = self.witness().home_owner();
        Delivered::lift(self.reseal(), home)
    }
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use super::*;
    use crate::witnessed::{reattachable, Sealed};

    struct TestProfile;

    impl StorageProfile for TestProfile {
        type Families = ();
        type FrameOwner = TestFrame;
    }

    /// A `FrameStorage` stand-in: a region, an `outer` ancestor link, and the eternal-tier flag,
    /// mirroring the shape every production `PinsRegion` frame owner shares.
    struct TestFrame {
        region: Region<TestProfile>,
        outer: Option<Rc<TestFrame>>,
        eternal: bool,
    }

    // SAFETY: the owned `Region`'s arena pages stay fixed-address while `self` is held (behind an
    // `Rc` at every use site).
    unsafe impl RegionOwner for TestFrame {
        type Region = Region<TestProfile>;
        fn region(&self) -> &Region<TestProfile> {
            &self.region
        }
    }

    // SAFETY: `pins_region` walks self's own region and its `outer` ancestor chain; holding self's
    // `Rc` holds each ancestor `Rc` in turn, so every region the walk reports pinned stays live.
    unsafe impl PinsRegion for TestFrame {
        fn pins_region(&self, region: &Region<TestProfile>) -> bool {
            let mut node = self;
            loop {
                if std::ptr::eq(&node.region, region) {
                    return true;
                }
                match &node.outer {
                    Some(outer) => node = outer,
                    None => return false,
                }
            }
        }

        fn needs_no_pin(&self) -> bool {
            self.eternal
        }
    }

    fn root_frame() -> Rc<TestFrame> {
        Rc::new_cyclic(|me| TestFrame {
            region: Region::new(me.clone()),
            outer: None,
            eternal: false,
        })
    }

    /// A frame at the eternal tier — storage that outlives every region, so a pin on it buys
    /// nothing ([`PinsRegion::needs_no_pin`]).
    fn eternal_frame() -> Rc<TestFrame> {
        Rc::new_cyclic(|me| TestFrame {
            region: Region::new(me.clone()),
            outer: None,
            eternal: true,
        })
    }

    /// A frame whose `outer` is `root` — an ancestor-chain pin, for the subsumption cases.
    fn child_frame(outer: &Rc<TestFrame>) -> Rc<TestFrame> {
        let outer = Rc::clone(outer);
        Rc::new_cyclic(|me| TestFrame {
            region: Region::new(me.clone()),
            outer: Some(outer),
            eternal: false,
        })
    }

    /// A borrow into some region's content — the value shape an `Opened` hands back.
    struct RefFamily;
    reattachable! { RefFamily => &'r u32 }

    /// Mint a description hosted in `home` naming every frame in `sources` — the resident-bind
    /// shape, whose returned bundle the caller would hold as its pins.
    fn mint_in<'a>(
        home: &'a Rc<TestFrame>,
        sources: &[&Rc<TestFrame>],
    ) -> (&'a ReachDescription<TestFrame>, PinBundle<TestFrame>) {
        let bundles: Vec<PinBundle<TestFrame>> = sources
            .iter()
            .map(|f| PinBundle::singleton(Rc::clone(f)))
            .collect();
        let refs: Vec<&PinBundle<TestFrame>> = bundles.iter().collect();
        ReachDescription::mint(RegionHandle::from_owner(&**home), &refs)
    }

    /// Seal `carrier` over a borrow of `value` — the caller then opens it under its own pin, which
    /// is the only route to a membership query, since only [`Opened`] can answer one.
    fn seal_ref(carrier: Carrier<TestFrame>, value: &u32) -> Sealed<RefFamily, Carrier<TestFrame>> {
        Sealed::seal(crate::witnessed::Witnessed::from_erased(
            Erased::erase(value),
            carrier,
        ))
    }

    #[test]
    fn mint_stamps_the_destination_owner_as_host() {
        let home = root_frame();
        let foreign = root_frame();
        let (minted, _bundle) = mint_in(&home, &[&foreign]);
        assert!(
            minted.with_home_region(|region| std::ptr::eq(region, home.region())),
            "the host is the owner of the region the description was minted into"
        );
    }

    #[test]
    fn a_region_pure_value_still_gets_a_hosted_description() {
        let home = root_frame();
        let (minted, bundle) = mint_in(&home, &[]);
        assert!(minted.is_empty(), "nothing composed in, so no member");
        assert!(bundle.is_empty());
        assert!(
            minted.with_home_region(|region| std::ptr::eq(region, home.region())),
            "residence is recorded even with an empty member set"
        );
    }

    #[test]
    fn clone_is_a_reference_copy_with_no_refcount_traffic() {
        let home = root_frame();
        let foreign = root_frame();
        let (minted, _bundle) = mint_in(&home, &[&foreign]);
        let c: Carrier<TestFrame> = Carrier::new(minted);
        let home_before = Rc::strong_count(&home);
        let foreign_before = Rc::strong_count(&foreign);
        let cloned = c;
        assert_eq!(Rc::strong_count(&home), home_before);
        assert_eq!(Rc::strong_count(&foreign), foreign_before);
        let _ = cloned;
        assert_eq!(Rc::strong_count(&foreign), foreign_before);
    }

    #[test]
    fn opened_answers_membership_under_its_own_pin() {
        let foreign = root_frame();
        let home = root_frame();
        // Mint a description naming `foreign` into `home`'s own region — a resident bind's shape.
        let (minted, _bundle) = mint_in(&home, &[&foreign]);
        let value = 7u32;
        let sealed = seal_ref(Carrier::new(minted), &value);
        let opened = sealed.open_at(&home);
        assert!(opened.reach_covers(foreign.region()));
        assert!(
            !opened.reach_covers(home.region()),
            "nothing composed home in, so it is not a member"
        );
        assert!(opened.has_reach_members());
    }

    #[test]
    fn opened_reads_residence_off_the_description() {
        let foreign = root_frame();
        let home = root_frame();
        let (minted, _bundle) = mint_in(&home, &[&foreign]);
        let value = 7u32;
        let sealed = seal_ref(Carrier::new(minted), &value);
        let opened = sealed.open_at(&home);
        assert!(opened.with_home_region(|region| std::ptr::eq(region, home.region())));
    }

    #[test]
    fn borrows_home_is_true_exactly_when_the_borrows_reach_the_home_region() {
        let home = root_frame();
        let foreign = root_frame();
        let value = 7u32;

        // Borrows into its own region: home rides the source bundle as an ordinary member.
        let (self_borrowing, _bundle) = mint_in(&home, &[&home]);
        let opened = seal_ref(Carrier::new(self_borrowing), &value);
        let opened = opened.open_at(&home);
        assert!(opened.borrows_home());

        // Borrows only a foreign region: resident in `home`, reaching nothing of it.
        let (foreign_only, _bundle) = mint_in(&home, &[&foreign]);
        let sealed = seal_ref(Carrier::new(foreign_only), &value);
        let opened = sealed.open_at(&home);
        assert!(!opened.borrows_home());
    }

    #[test]
    fn a_region_pure_value_does_not_borrow_its_home() {
        let home = root_frame();
        let value = 7u32;
        let (minted, _bundle) = mint_in(&home, &[]);
        let sealed = seal_ref(Carrier::new(minted), &value);
        let opened = sealed.open_at(&home);
        assert!(
            !opened.borrows_home(),
            "living in a region is not borrowing into it"
        );
        assert!(!opened.has_reach_members());
        assert!(!opened.reach_covers(home.region()));
    }

    #[test]
    fn reseal_round_trip_preserves_the_reach_pairing() {
        let foreign = root_frame();
        let home = root_frame();
        let (minted, _bundle) = mint_in(&home, &[&foreign]);
        let value = 7u32;
        let sealed = seal_ref(Carrier::new(minted), &value);
        let opened = sealed.open_at(&home);
        assert!(opened.reach_covers(foreign.region()));
        let resealed = opened.reseal();
        let reopened = resealed.open_at(&home);
        assert!(
            reopened.reach_covers(foreign.region()),
            "the value↔reach pairing survives the reseal/re-open round trip"
        );
        assert!(
            reopened.with_home_region(|region| std::ptr::eq(region, home.region())),
            "and so does the residence"
        );
    }

    #[test]
    fn pins_beyond_eternal_filters_the_eternal_tier() {
        let home = root_frame();
        let eternal = eternal_frame();
        let mortal = root_frame();

        let (eternal_only, _bundle) = mint_in(&home, &[&eternal]);
        assert!(
            !eternal_only.pins_beyond_eternal(),
            "storage that outlives every region is not a reach worth relocating for"
        );

        let (mixed, _bundle) = mint_in(&home, &[&eternal, &mortal]);
        assert!(
            mixed.pins_beyond_eternal(),
            "one mortal member is enough — that is the region a per-call frame takes with it"
        );

        let (pure, _bundle) = mint_in(&home, &[]);
        assert!(
            !pure.pins_beyond_eternal(),
            "no members, nothing to outlive"
        );
    }

    #[test]
    fn mint_keeps_dest_in_description_and_drops_it_from_the_bundle() {
        let dest = root_frame();
        let foreign = root_frame();
        let handle = RegionHandle::from_owner(&*dest);
        let source = PinBundle::union(
            &PinBundle::singleton(Rc::clone(&dest)),
            &PinBundle::singleton(Rc::clone(&foreign)),
        );
        let (set, bundle) = ReachDescription::mint(handle, &[&source]);
        assert!(
            set.pins_region(dest.region()),
            "membership is exact: the description names dest's own region"
        );
        assert!(
            !bundle.members().iter().any(|m| Rc::ptr_eq(m, &dest)),
            "the self rule strips dest from the owned bundle — a region never pins itself"
        );
        assert!(bundle.pins_region(foreign.region()));
    }

    #[test]
    fn mint_keeps_a_dest_ancestor_in_the_bundle() {
        let outer = root_frame();
        let dest = child_frame(&outer);
        let (set, bundle) = mint_in(&dest, &[&outer]);
        assert!(set.pins_region(outer.region()));
        assert!(
            bundle.members().iter().any(|m| Rc::ptr_eq(m, &outer)),
            "the self rule strips dest's own region only — an ancestor closes no cycle, \
             and holding it does not keep dest alive, so it must survive"
        );
    }

    #[test]
    fn mint_lets_dest_subsume_its_own_ancestor_out_of_the_bundle() {
        let outer = root_frame();
        let dest = child_frame(&outer);
        let handle = RegionHandle::from_owner(&*dest);
        let source = PinBundle::union(
            &PinBundle::singleton(Rc::clone(&dest)),
            &PinBundle::singleton(Rc::clone(&outer)),
        );
        let (set, bundle) = ReachDescription::mint(handle, &[&source]);
        assert!(
            set.pins_region(outer.region()),
            "subsumption keeps dest, whose chain still reports the ancestor pinned"
        );
        assert!(
            bundle.is_empty(),
            "dest subsumes its own ancestor, then the self rule strips dest — the holder is dest \
             itself, whose liveness already covers the whole chain"
        );
    }

    #[test]
    fn mint_forwards_reach_members() {
        let foreign = root_frame();
        let home = root_frame();
        let dest = root_frame();
        let (source, reach) = mint_in(&home, &[&foreign]);
        let c: Carrier<TestFrame> = Carrier::new(source);
        // The holder's pins are home ∪ reach — the envelope shape, home as an ordinary member.
        let pins = PinBundle::union(&PinBundle::singleton(Rc::clone(&home)), &reach);
        let dest_handle = RegionHandle::from_owner(&*dest);
        let (set, bundle) = c.mint_into(&pins, dest_handle);
        assert!(set.pins_region(foreign.region()));
        assert!(
            set.pins_region(home.region()),
            "home rides the source bundle like any other member"
        );
        assert!(
            set.with_home_region(|region| std::ptr::eq(region, dest.region())),
            "the product lives where it was minted"
        );
        assert!(bundle.pins_region(home.region()));
    }
}
