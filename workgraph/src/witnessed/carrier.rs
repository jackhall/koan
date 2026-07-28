//! [`Carrier<F>`] — the reference-only carrier witness: a *reference* to a hosted reach set plus
//! the `borrows_host` bit, the same shape whether the value is resident in a region or walking
//! between nodes. See
//! [design/witness-hosting.md § The carrier states](../../../design/witness-hosting.md#the-carrier-states).
//!
//! The carrier **owns no pin**: cloning is a bit-copy plus a reference-copy, and a carrier's death
//! releases nothing. What keeps its reach set (and the value it describes) alive is external —
//! the container's liveness when resident, the scheduler's frame-retention hold (travelling as the
//! [`Delivered`](super::Delivered) envelope) when walking — so every re-anchor of the erased reach
//! reference happens under a named pin. The set a carrier references is never owned by it:
//! [`ReachDescription::mint`] is the only way such a description comes to exist, and it always
//! lands in the value's home region's own side table, so whatever covers the home region covers the
//! reference. The description is non-owning (its members are `Weak`); the owned [`PinBundle`] that
//! pins the reached regions is held by the value's holder (a binding entry, the delivery envelope),
//! never by the carrier.
//!
//! A value's **home is an ordinary member** of the set its carrier references: the envelope's pins
//! carry it like any other reached region, so a mint composes it with no separate materialization
//! arm and no residence mode. The one asymmetry is the self rule at the owned-upgrade boundary — a
//! region never pins itself — which [`ReachDescription::mint`] applies to the owned bundle alone.
//!
//! `Carrier` is deliberately **not** a [`super::Witness`]: a bare [`super::Sealed::open`] under it
//! does not compile. Reads name their coverage — [`super::Sealed::open_with`] under an external
//! pin, the envelope's [`Delivered::open`](super::Delivered::open), or the borrow-tied
//! [`Opened`](super::Opened) state, which is the only state that answers a membership query —
//! and relocations run through the envelope-bearing mint verbs
//! ([`Delivered::mint_reach`](super::Delivered::mint_reach),
//! [`Delivered::transfer_into`](super::Delivered::transfer_into)). [`Self::mint_into`] is the
//! crate-internal core they route through; it is not part of the public surface.

use std::rc::Rc;

use super::{
    Erased, Opened, PinBundle, PinsRegion, ReachDescription, Reattachable, Region, RegionHandle,
    RegionOwner, StorageProfile,
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

/// The reference-only carrier witness: a reference to a reach set living in the value's home
/// region's own arena, plus the `borrows_host` bit. `F` is the workload's frame-owner
/// type (`Rc<F>` is the home pin the *envelope*, not the carrier, holds). Clone is a bit-copy plus
/// a reference-copy — no refcount traffic; the carrier keeps nothing alive.
pub struct Carrier<F: PinsRegion + 'static> {
    /// Whether the value's borrows reach into its own home region. **Not** derivable from `reach`:
    /// a value born borrowing only its birth region carries `borrows_host` with `reach: None`, a
    /// state no membership query can express because there is no description to hold home as a
    /// member. That is the shape an embedder's birth-site force mints (koan's
    /// `force_substrate_borrows_host`), and [`Self::is_empty`] is what reads it — so dropping the
    /// bit would report such a value as reaching nothing at all. Retiring it means making every
    /// birth site mint a description naming home instead
    /// ([roadmap](../../../roadmap/untyped_arena/home-lives-in-the-reach-description.md)).
    borrows_host: bool,
    /// The value's foreign reach, erased and re-anchored only under an externally supplied pin.
    /// `None` == the empty set (encoded without an allocation).
    reach: Option<Erased<HostedSetRef<F>>>,
}

impl<F: PinsRegion + 'static> Default for Carrier<F> {
    /// The frameless / region-pure carrier: no borrows into home, empty reach. Every koan-side
    /// construction site builds this; reach-carrying carriers are library-minted (or reference a
    /// binding's library-minted set through [`Carrier::new`]).
    fn default() -> Self {
        Carrier {
            borrows_host: false,
            reach: None,
        }
    }
}

impl<F: PinsRegion + 'static> Clone for Carrier<F> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<F: PinsRegion + 'static> Copy for Carrier<F> {}

impl<F: PinsRegion + 'static> Carrier<F> {
    /// Reference an already-minted reach set (with its `borrows_host` bit) as a carrier — the
    /// resident-read constructor: a binding entry stores the set reference and bit this rebuilds a
    /// read carrier from. The set was minted by the library at bind time into the value's home
    /// arena; this constructor only re-packages the reference, so reach totality still rests on
    /// the mint. The carrier pins nothing — the read that re-anchors `reach` names its pin there.
    pub fn new(borrows_host: bool, reach: Option<&ReachDescription<F>>) -> Self {
        Carrier {
            borrows_host,
            reach: reach.map(Erased::erase),
        }
    }

    /// Whether the value's borrows reach into its own home region. When the carrier references a
    /// description, home is an ordinary member of it and this agrees with the membership query
    /// ([`Opened::reach_covers`]) — pin-free, for a call site holding only the carrier. When it
    /// references none, the bit is the *only* record of the home borrow: see the field.
    pub fn borrows_host(&self) -> bool {
        self.borrows_host
    }

    /// Whether this carrier represents no reach at all — the frameless / region-pure terminal,
    /// whose backing outlives the carrier so no pin or mint is ever needed for it.
    pub fn is_empty(&self) -> bool {
        !self.borrows_host && self.reach.is_none()
    }

    /// Whether this carrier references a reach description at all — i.e. whether the value reaches
    /// any region, its own home included. Distinct from [`Self::is_empty`], which also requires the
    /// home-borrow bit unset. The empty-pins Done-arm seal keys on this, not on `is_empty`.
    pub fn has_reach_members(&self) -> bool {
        self.reach.is_some()
    }

    /// Upgrade this carrier's reach description into an owned [`PinBundle`] under `pin` — the
    /// `Weak → Rc` upgrade the [`Sealed → Delivered`](super::Sealed) **lift** routes: a value read
    /// out of an arena-hosted seal is re-owned so the source frame may die in transit while the
    /// bundle keeps its reached regions alive. `pin` covers the description's hosting arena for the
    /// whole call (the holder rule), so [`ReachDescription::to_bundle`]'s member upgrades all
    /// succeed. `None` reach (the empty / region-pure carrier) yields the empty bundle.
    pub(in crate::witnessed) fn upgrade_bundle(&self, pin: &Rc<F>) -> PinBundle<F> {
        // `pin` keeps the description's hosting arena live for the whole call — the same role the
        // envelope host plays for a reach read; the branded re-anchor confines the reference exactly
        // as `with_reach_impl` does, and the upgrade re-owns the members before it ends.
        let _ = pin;
        match &self.reach {
            None => PinBundle::empty(),
            Some(erased) => with_branded_ref::<HostedSetRef<F>, _>(
                erased.as_static(),
                |set_ref: &&ReachDescription<F>| set_ref.to_bundle(),
            ),
        }
    }

    /// Read the reach set this carrier references, re-anchored for the call. The crate-internal
    /// core behind [`Opened::with_reach`] — the caller is responsible for the coverage the
    /// re-anchor needs, which is why the only public route in is through the [`Opened`] state,
    /// whose `'b` **is** the pin borrow. `None` reach means the empty set.
    fn with_reach_impl<R>(&self, f: impl FnOnce(Option<&ReachDescription<F>>) -> R) -> R {
        match &self.reach {
            None => f(None),
            Some(erased) => {
                with_branded_ref::<HostedSetRef<F>, R>(erased.as_static(), |set_ref: &&_| {
                    f(Some(*set_ref))
                })
            }
        }
    }

    /// Mint this carrier's reach into `dest` and report whether the value's borrows reach `dest`'s
    /// own region — the shared core the crate-internal mint verbs route through
    /// ([`Delivered::mint_reach`](super::Delivered::mint_reach)). Not itself part of the public
    /// surface. Applies, via [`ReachDescription::mint`]: outer-chain subsumption and the self rule
    /// on the returned bundle — no caller policy, so the minted description is exact.
    ///
    /// `source` is the holder's owned pin bundle — the delivery envelope's pins, a binding entry's
    /// pins — which names the value's home as an ordinary member alongside everything else it
    /// reaches. It is threaded in, never recovered from the carrier's description, so the
    /// composition folds strong `Rc`s. Returns the minted description (`None` == empty, no
    /// allocation) hosted in `dest`, the owned [`PinBundle`] the caller keeps to pin its members,
    /// and the borrows-into-dest bit.
    pub(in crate::witnessed) fn mint_into<'d, P>(
        &self,
        source: &PinBundle<F>,
        dest: RegionHandle<'d, P>,
    ) -> (Option<&'d ReachDescription<F>>, PinBundle<F>, bool)
    where
        P: StorageProfile<FrameOwner = F> + 'static,
        F: RegionOwner<Region = Region<P>>,
    {
        ReachDescription::mint_with_dest_bit(dest, &[source])
    }

    /// The relocation composition behind the envelope's
    /// [`transfer_into`](super::Delivered::transfer_into) and the live-carrier reach merges: mint
    /// BOTH operands' exact reach — the destination's (an accumulator's prior folds, threaded as
    /// `right_bundle`) and the newly-folded source's (`left_bundle`) — into `dest`'s arena, and
    /// compute the composed borrows-into-dest bit. Never the source alone, or a multi-step
    /// accumulator fold would drop everything folded before this step. Both operand bundles are
    /// owned and threaded in — the composition folds strong `Rc`s, never a description's `Weak` —
    /// and `left_bundle` is where the folded value's home enters, as an ordinary member.
    ///
    /// The source carrier is not an input: its `borrows_host` bit says its borrows reach its own
    /// home, which contributes to *this* destination only when that home is `dest` — and then
    /// `left_bundle` names `dest` and the query below reports it. The destination carrier's `right`
    /// bit is not subsumed the same way, because the self rule has already stripped `dest` from
    /// `right_bundle`.
    ///
    /// Returns the composed carrier paired with the freshly-minted owned bundle: `dest`'s region
    /// **retains a clone** for the region's life (what keeps the relocated value's reach alive when
    /// the product is consumed in place — read directly rather than re-enveloped), and the returned
    /// bundle threads to the next fold step or the terminal seal.
    pub(in crate::witnessed) fn compose_into<'b, P>(
        right: &Self,
        left_bundle: &PinBundle<F>,
        right_bundle: &PinBundle<F>,
        dest: RegionHandle<'b, P>,
    ) -> (Self, PinBundle<F>)
    where
        P: StorageProfile<FrameOwner = F> + 'static,
        F: RegionOwner<Region = Region<P>>,
    {
        let (minted, bundle) = ReachDescription::mint(dest, &[left_bundle, right_bundle]);
        dest.region().retain_reach(bundle.clone());
        let borrows_into_dest = right.borrows_host || left_bundle.pins_region(dest.region());
        let carrier = Carrier {
            borrows_host: borrows_into_dest,
            reach: minted.map(Erased::<HostedSetRef<F>>::erase),
        };
        (carrier, bundle)
    }
}

/// The membership queries, on the **in-use** carrier state: an [`Opened`] borrows at `'b` under the
/// pin that was presented to open it, and that borrow is exactly the coverage re-anchoring the
/// erased reach reference requires — so these need no `pin` argument, and there is no way to ask
/// the question without one (design/witness-hosting.md § The carrier states).
impl<'b, T: Reattachable, F: PinsRegion + 'static> Opened<'b, T, Carrier<F>> {
    /// Whether the value's reach names `region` — home included, since home is an ordinary member.
    pub fn reach_covers(&self, region: &F::Region) -> bool {
        self.with_reach(|reach| reach.is_some_and(|r| r.pins_region(region)))
    }

    /// Read the reach description this open's carrier references, re-anchored under the open's own
    /// `'b` pin borrow. `None` means the empty set — a region-pure value, which names nothing.
    pub fn with_reach<R>(&self, f: impl FnOnce(Option<&ReachDescription<F>>) -> R) -> R {
        self.witness().with_reach_impl(f)
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

    /// A `FrameStorage` stand-in: a region plus an `outer` ancestor link, mirroring the shape every
    /// production `PinsRegion` frame owner shares.
    struct TestFrame {
        region: Region<TestProfile>,
        outer: Option<Rc<TestFrame>>,
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
    }

    fn root_frame() -> Rc<TestFrame> {
        Rc::new(TestFrame {
            region: Region::new(),
            outer: None,
        })
    }

    /// A frame whose `outer` is `root` — an ancestor-chain pin, for the subsumption cases.
    fn child_frame(outer: &Rc<TestFrame>) -> Rc<TestFrame> {
        Rc::new(TestFrame {
            region: Region::new(),
            outer: Some(Rc::clone(outer)),
        })
    }

    /// A borrow into some region's content — the value shape an `Opened` hands back.
    struct RefFamily;
    reattachable! { RefFamily => &'r u32 }

    /// Seal `carrier` over a borrow of `value` — the caller then opens it under its own pin, which
    /// is the only route to a membership query, since only [`Opened`] can answer one.
    fn seal_ref(carrier: Carrier<TestFrame>, value: &u32) -> Sealed<RefFamily, Carrier<TestFrame>> {
        Sealed::seal(crate::witnessed::Witnessed::from_erased(
            Erased::erase(value),
            carrier,
        ))
    }

    #[test]
    fn default_is_empty() {
        let c: Carrier<TestFrame> = Carrier::default();
        assert!(c.is_empty());
        assert!(!c.borrows_host());
        assert!(!c.has_reach_members());
    }

    #[test]
    fn clone_is_a_bit_copy_with_no_refcount_traffic() {
        let home = root_frame();
        let foreign = root_frame();
        let handle = RegionHandle::from_owner(&*home);
        let (minted, _bundle) =
            ReachDescription::mint(handle, &[&PinBundle::singleton(Rc::clone(&foreign))]);
        let c: Carrier<TestFrame> = Carrier::new(false, minted);
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
        // Mint a set naming `foreign` into `home`'s own region — the shape a resident bind produces.
        let handle = RegionHandle::from_owner(&*home);
        let (minted, _bundle) =
            ReachDescription::mint(handle, &[&PinBundle::singleton(Rc::clone(&foreign))]);
        let value = 7u32;
        let sealed = seal_ref(Carrier::new(false, minted), &value);
        let opened = sealed.open_at(&home);
        assert!(opened.reach_covers(foreign.region()));
        assert!(
            !opened.reach_covers(home.region()),
            "nothing composed home in, so it is not a member"
        );
    }

    #[test]
    fn empty_carrier_reaches_nothing() {
        let frame = root_frame();
        let value = 7u32;
        let sealed = seal_ref(Carrier::default(), &value);
        let opened = sealed.open_at(&frame);
        assert!(!opened.reach_covers(frame.region()));
        opened.with_reach(|reach| assert!(reach.is_none()));
    }

    #[test]
    fn reseal_round_trip_preserves_the_reach_pairing() {
        let foreign = root_frame();
        let home = root_frame();
        let handle = RegionHandle::from_owner(&*home);
        let (minted, _bundle) =
            ReachDescription::mint(handle, &[&PinBundle::singleton(Rc::clone(&foreign))]);
        let value = 7u32;
        let sealed = seal_ref(Carrier::new(false, minted), &value);
        let opened = sealed.open_at(&home);
        assert!(opened.reach_covers(foreign.region()));
        let resealed = opened.reseal();
        let reopened = resealed.open_at(&home);
        assert!(
            reopened.reach_covers(foreign.region()),
            "the value↔reach pairing survives the reseal/re-open round trip"
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
        let (minted, bundle) = ReachDescription::mint(handle, &[&source]);
        let set = minted.expect("the composed reach is non-empty");
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
        let handle = RegionHandle::from_owner(&*dest);
        let (minted, bundle) =
            ReachDescription::mint(handle, &[&PinBundle::singleton(Rc::clone(&outer))]);
        let set = minted.expect("the ancestor is a member");
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
        let (minted, bundle) = ReachDescription::mint(handle, &[&source]);
        let set = minted.expect("dest survives into the description");
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
    fn mint_reports_borrows_into_dest_from_the_source_query() {
        let dest = root_frame();
        let c: Carrier<TestFrame> = Carrier::default();
        let handle = RegionHandle::from_owner(&*dest);
        // The value's home IS the destination, and home rides the source bundle as an ordinary
        // member — so the pre-strip source query is what reports the bit.
        let (_, _bundle, borrows_into_dest) =
            c.mint_into(&PinBundle::singleton(Rc::clone(&dest)), handle);
        assert!(borrows_into_dest);
    }

    #[test]
    fn mint_reports_no_borrow_into_a_foreign_dest() {
        let home = root_frame();
        let dest = root_frame();
        let c: Carrier<TestFrame> = Carrier::default();
        let handle = RegionHandle::from_owner(&*dest);
        let (_, _bundle, borrows_into_dest) =
            c.mint_into(&PinBundle::singleton(Rc::clone(&home)), handle);
        assert!(!borrows_into_dest);
    }

    #[test]
    fn mint_forwards_reach_members() {
        let foreign = root_frame();
        let home = root_frame();
        let dest = root_frame();
        let home_handle = RegionHandle::from_owner(&*home);
        let (source, reach) =
            ReachDescription::mint(home_handle, &[&PinBundle::singleton(Rc::clone(&foreign))]);
        let c: Carrier<TestFrame> = Carrier::new(false, source);
        // The holder's pins are home ∪ reach — the envelope shape, home as an ordinary member.
        let pins = PinBundle::union(&PinBundle::singleton(Rc::clone(&home)), &reach);
        let dest_handle = RegionHandle::from_owner(&*dest);
        let (minted, bundle, _) = c.mint_into(&pins, dest_handle);
        let set = minted.expect("reach members always mint forward");
        assert!(set.pins_region(foreign.region()));
        assert!(
            set.pins_region(home.region()),
            "home rides the source bundle like any other member"
        );
        assert!(bundle.pins_region(home.region()));
    }
}
