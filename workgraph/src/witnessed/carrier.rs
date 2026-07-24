//! [`Carrier<F>`] — the reference-only carrier witness: one `borrows_host` bit plus a *reference*
//! to a hosted reach set, the same shape whether the value is resident in a region or walking
//! between nodes. See
//! [design/witness-hosting.md § The carrier](../../../design/witness-hosting.md#the-carrier).
//!
//! The carrier **owns no pin**: cloning is a bit-copy plus a reference-copy, and a carrier's death
//! releases nothing. What keeps its reach set (and the value it describes) alive is external —
//! the container's liveness when resident, the scheduler's frame-retention hold (travelling as the
//! [`Delivered`](super::Delivered) envelope) when walking — so every re-anchor of the erased reach
//! reference names its pin at the read ([`Carrier::with_reach`]). The set a carrier references is
//! never owned by it: [`ReachDescription::mint`] is the only way such a description comes to exist,
//! and it always lands in the value's host region's own side table, so whatever covers the host
//! region covers the reference. The description is non-owning (its members are `Weak`); the owned
//! [`PinBundle`] that pins the reached regions is held by the value's holder (a binding entry, the
//! delivery envelope), never by the carrier.
//!
//! `Carrier` is deliberately **not** a [`super::Witness`]: a bare [`super::Sealed::open`] under it
//! does not compile. Reads name their coverage — [`super::Sealed::open_with`] under an external
//! pin, or the envelope's [`Delivered::open`](super::Delivered::open) — and relocations run
//! through the envelope-bearing mint verbs ([`Delivered::mint_reach`](super::Delivered::mint_reach),
//! [`Delivered::transfer_into`](super::Delivered::transfer_into)), the only places a residence
//! host materializes as a set member. A value with no envelope — already resident in a region the
//! caller's context covers ambiently — mints through [`Witnessed::mint_resident_reach`] instead,
//! the envelope-free twin. [`Self::mint_into`] is the crate-internal core both route through; it is
//! not part of the public surface.

use std::rc::Rc;

use super::{
    Erased, FoldToken, FoldedPlacement, PinBundle, PinsRegion, ReachDescription, Reattachable,
    Region, RegionHandle, RegionOwner, StorageProfile, Witness, Witnessed,
};
// `with_branded_ref` re-anchors the erased reach reference inside `with_reach_impl`, which is a
// white-box test hook now that no production path reads a carrier's description.
#[cfg(any(test, feature = "test-hooks"))]
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

/// The residence mode of a re-home mint: did the value **keep living** in the producer's region
/// (a copy-free re-anchor), or was it **copied out** into the destination? Decides whether the
/// producer host materializes as a member of the minted set unconditionally (`Kept` — residence
/// itself must stay pinned) or only when the value's borrows genuinely reach it (`Copied` — the
/// `borrows_host` bit; a residence-only host is dropped, freeing the producer at retention
/// release). Policy is the embedder's: each adoption site names its mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Residence {
    /// The value keeps living in the producer's region — the copy-free re-anchor. The host always
    /// materializes as a member of the minted destination set.
    Kept,
    /// The value was copied out into the destination. The host materializes only if the value's
    /// borrows genuinely reach it (`borrows_host`).
    Copied,
    /// The value was copied out into the destination AND the embedder has independently verified
    /// that no surviving borrow leaf points back into the producer's region (an exact,
    /// per-value check the embedder runs before choosing this over `Copied`). The host never
    /// materializes, unconditionally — unlike `Copied`, not even a set `borrows_host` bit
    /// resurrects it — and the composed `borrows_into_dest` bit drops the host-pin term along
    /// with it: post-copy, the value does not borrow the host.
    Released,
}

/// The reference-only carrier witness: the `borrows_host` bit plus (for a value with foreign
/// reach) a reference to a reach set living in the value's host region's own arena. `F` is the
/// workload's frame-owner type (`Rc<F>` is the residence pin the *envelope*, not the carrier,
/// holds). Clone is a bit-copy plus a reference-copy — no refcount traffic; the carrier keeps
/// nothing alive.
pub struct Carrier<F: PinsRegion + 'static> {
    /// Whether the value's borrows reach into its own home region (materialized separately from
    /// `reach`, which is home-omitted by construction).
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

    /// Whether the value's borrows reach into its own home region — the bit consumed at exactly
    /// one kind of site, the re-home mint into a different destination arena (`Residence::Copied`
    /// materialization). Never a lifecycle input.
    ///
    /// White-box reach introspection: production code ([`Self::mint_into`] / [`Self::compose_into`])
    /// reads the `borrows_host` field directly, so this accessor has no library-internal caller and
    /// is gated entirely behind `test-hooks` for an embedder's white-box tests (mirroring
    /// `Scheduler::anchor_of`'s gate) rather than split into a `pub(crate)` core.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn borrows_host(&self) -> bool {
        self.borrows_host
    }

    /// Whether this carrier represents no reach at all — the frameless / region-pure terminal,
    /// whose backing outlives the carrier so no pin or mint is ever needed for it.
    pub fn is_empty(&self) -> bool {
        !self.borrows_host && self.reach.is_none()
    }

    /// Whether the value's borrows reach a **foreign** region — i.e. whether the value's owned pin
    /// bundle is non-empty. Distinct from [`Self::is_empty`], which also requires the home-borrow
    /// bit (`borrows_host`) unset: a value that borrows only its own home (a fresh closure capturing
    /// its defining scope) reaches nothing foreign, so its owned pins are empty even though it is not
    /// `is_empty`. The empty-pins Done-arm seal ([`StepCarried::born`](crate::witnessed) veneer) keys
    /// on this, not on `is_empty`.
    pub fn has_foreign_reach(&self) -> bool {
        self.reach.is_some()
    }

    /// Read the reach set this carrier references, re-anchored under `pin` — the sole re-anchor of
    /// the erased reach reference. `None` reach means the empty set.
    ///
    /// `pin` names the coverage of the set's hosting arena — the value's retained frame owner (the
    /// [`Delivered`](super::Delivered) envelope's host, the retention hold) — and is held for the
    /// whole call, so the re-anchored reference cannot dangle; the closure confines it exactly as
    /// [`super::Sealed::open_with`] confines a value. Pass `pin: None` only when the hosting arena
    /// is covered by the caller's ambient container — the reader's own region for a resident
    /// carrier's set, or the step pin held across a step-brand read.
    #[cfg(any(test, feature = "test-hooks"))]
    fn with_reach_impl<R>(
        &self,
        pin: Option<&Rc<F>>,
        f: impl FnOnce(Option<&ReachDescription<F>>) -> R,
    ) -> R {
        let _ = pin;
        match &self.reach {
            None => f(None),
            Some(erased) => {
                with_branded_ref::<HostedSetRef<F>, R>(erased.as_static(), |set_ref: &&_| {
                    f(Some(*set_ref))
                })
            }
        }
    }

    /// White-box reach introspection, exposed `pub` only under `test-hooks` (or the crate's own
    /// tests) for an embedder's white-box tests — mirroring `Scheduler::anchor_of`'s gate. Ownership
    /// now flows forward from the mint, so no production path reads a carrier's description to build
    /// a bundle; the membership queries alone survive, and only tests observe them.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn with_reach<R>(
        &self,
        pin: Option<&Rc<F>>,
        f: impl FnOnce(Option<&ReachDescription<F>>) -> R,
    ) -> R {
        self.with_reach_impl(pin, f)
    }

    /// Whether the value's foreign reach names `region` — reach members only; the borrows-into-home
    /// bit is a separate query ([`Self::borrows_host`]) because the home it refers to is the
    /// envelope's knowledge, not the carrier's. `pin` covers the reach set's hosting arena, as in
    /// [`Self::with_reach`]. Same `test-hooks` gate as [`Self::with_reach`].
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn reach_covers(&self, pin: Option<&Rc<F>>, region: &F::Region) -> bool {
        self.with_reach_impl(pin, |reach| reach.is_some_and(|r| r.pins_region(region)))
    }

    /// Mint this carrier's reach into `dest`, materializing `host` (the value's producer frame
    /// owner) per `mode`, and report whether the value's borrows reach `dest`'s own region — the
    /// shared core both crate-internal mint verbs route through:
    /// [`Delivered::mint_reach`](super::Delivered::mint_reach) supplies `Some(host)` for an
    /// envelope-bearing value, and [`Witnessed::mint_resident_reach`] supplies `None` for a value
    /// already resident in a region the caller's context covers ambiently. Not itself part of the
    /// public surface. Applies, via [`ReachDescription::mint`]: home-omission (`dest`'s own region
    /// is never a member), the caller's `omit` policy predicate, and outer-chain subsumption.
    ///
    /// `source` is this carrier's own owned foreign reach bundle, threaded from the holder (the
    /// delivery envelope's `foreign`, a binding entry's pins) — never recovered from the carrier's
    /// description, so the composition folds strong `Rc`s. `host` is the value's producer frame
    /// owner, materialized per `mode`; `None` asserts there is no residence to materialize (a
    /// resident value's own region), so `mode` only gates a `Some` host. Returns the minted
    /// description (`None` == empty, no allocation) hosted in `dest`, the owned [`PinBundle`] the
    /// caller keeps to pin its members, and the borrows-into-dest bit: reach members pinning
    /// `dest`'s region, or the `borrows_host` bit when `host` itself pins it (the value's home *is*
    /// — or subsumes — the destination).
    pub(in crate::witnessed) fn mint_into<'d, P>(
        &self,
        source: &PinBundle<F>,
        dest: RegionHandle<'d, P>,
        host: Option<&Rc<F>>,
        mode: Residence,
        omit: impl Fn(&Region<P>) -> bool,
    ) -> (Option<&'d ReachDescription<F>>, PinBundle<F>, bool)
    where
        P: StorageProfile<FrameOwner = F> + 'static,
        F: RegionOwner<Region = Region<P>>,
    {
        let materialize = materialize_hosts(host, mode, self.borrows_host);
        let (minted, bundle) = ReachDescription::mint(dest, &[source], &materialize, omit);
        let borrows_into_dest = source.pins_region(dest.region())
            || (mode != Residence::Released
                && self.borrows_host
                && host.is_some_and(|h| h.pins_region(dest.region())));
        (minted, bundle, borrows_into_dest)
    }

    /// The relocation composition behind the envelope's
    /// [`transfer_into`](super::Delivered::transfer_into) and the live-carrier reach merges: mint
    /// BOTH operands' exact reach — `right`'s (an accumulator's prior folds, threaded as
    /// `right_bundle`) and `left`'s (the newly-folded source, `left_bundle`) — plus `left`'s `host`
    /// per `mode`, into `dest`'s arena, and compute the composed borrows-into-dest bit. Never `left`
    /// alone, or a multi-step accumulator fold would drop everything folded before this step. Both
    /// operand bundles are owned and threaded in — the composition folds strong `Rc`s, never a
    /// description's `Weak`.
    ///
    /// Returns the composed carrier paired with the freshly-minted owned bundle: `dest`'s region
    /// **retains a clone** for the region's life (what keeps the relocated value's reach alive when
    /// the product is consumed in place — read directly rather than re-enveloped), and the returned
    /// bundle threads to the next fold step or the terminal seal.
    pub(in crate::witnessed) fn compose_into<'b, P>(
        left: &Self,
        right: &Self,
        left_bundle: &PinBundle<F>,
        right_bundle: &PinBundle<F>,
        dest: RegionHandle<'b, P>,
        host: Option<&Rc<F>>,
        mode: Residence,
    ) -> (Self, PinBundle<F>)
    where
        P: StorageProfile<FrameOwner = F> + 'static,
        F: RegionOwner<Region = Region<P>>,
    {
        let materialize = materialize_hosts(host, mode, left.borrows_host);
        let (minted, bundle) =
            ReachDescription::mint(dest, &[left_bundle, right_bundle], &materialize, |_| false);
        dest.region().retain_reach(bundle.clone());
        let borrows_into_dest = right.borrows_host
            || left_bundle.pins_region(dest.region())
            || (mode != Residence::Released
                && left.borrows_host
                && host.is_some_and(|h| h.pins_region(dest.region())));
        let carrier = Carrier {
            borrows_host: borrows_into_dest,
            reach: minted.map(Erased::<HostedSetRef<F>>::erase),
        };
        (carrier, bundle)
    }
}

/// The envelope-free mint entry: a value carried under a reference-only [`Carrier`] witness with no
/// [`Delivered`](super::Delivered) envelope in hand, because it already lives in a region the
/// caller's context covers ambiently (the run-teardown rehome path). Resident twin of
/// [`Delivered::mint_reach`](super::Delivered::mint_reach).
impl<T: Reattachable, F: PinsRegion + 'static> Witnessed<T, Carrier<F>> {
    /// Mints this resident value's reach into `dest`. `source` is the value's own owned foreign
    /// reach bundle, threaded from its holder — never recovered from the carrier's description.
    ///
    /// Resident twin of [`Delivered::mint_reach`](super::Delivered::mint_reach): the value already
    /// lives in a region the caller's context covers ambiently, so there is no residence host to
    /// materialize and no `Residence` mode to choose.
    pub fn mint_resident_reach<'d, P>(
        &self,
        source: &PinBundle<F>,
        dest: RegionHandle<'d, P>,
        omit: impl Fn(&Region<P>) -> bool,
    ) -> (Option<&'d ReachDescription<F>>, PinBundle<F>, bool)
    where
        P: StorageProfile<FrameOwner = F> + 'static,
        F: RegionOwner<Region = Region<P>>,
    {
        // `host: None` means there is no residence host to materialize, so `Residence::Kept` here
        // is arbitrary — `mint_into`'s `materialize_hosts` returns an empty vec under either mode
        // once `host` is `None`.
        self.witness()
            .mint_into(source, dest, None, Residence::Kept, omit)
    }

    /// Merge two **live** reference-only carriers under an externally supplied `pin` — the
    /// bundle-threading twin of [`Witnessed::merge_pinned`] for the [`Carrier`] witness, whose
    /// reach composition (unlike the self-contained generic [`super::ComposeWitness`]) folds owned
    /// bundles. `self_bundle` / `other_bundle` are the two operands' owned foreign reach bundles
    /// (threaded from their holders — an accumulator's prior fold, a resident read's entry pins);
    /// there is no residence host to fold (both operands are resident, `host: None` /
    /// `Residence::Copied`), so this is the pure reach mint. Returns the composed carrier paired with
    /// the freshly-minted owned bundle, to thread onward or seal.
    pub fn merge_reach<B, P, Pr, Pin>(
        self,
        self_bundle: &PinBundle<F>,
        other: Witnessed<B, Carrier<F>>,
        other_bundle: &PinBundle<F>,
        pin: &Pin,
        f: impl for<'b> FnOnce(T::At<'b>, B::At<'b>, FoldToken<'b>) -> P::At<'b>,
    ) -> (Witnessed<P, Carrier<F>>, PinBundle<F>)
    where
        B: Reattachable,
        P: Reattachable,
        Pin: Witness,
        Pr: StorageProfile<FrameOwner = F> + 'static,
        F: RegionOwner<Region = Region<Pr>>,
        for<'b> B::At<'b>: super::HasRegionHandle<'b, Pr>,
    {
        self.merge_composed(
            other,
            pin,
            |left, right, live_dest| {
                Carrier::compose_into(
                    left,
                    right,
                    self_bundle,
                    other_bundle,
                    live_dest.region_handle(),
                    None,
                    Residence::Copied,
                )
            },
            f,
        )
    }

    /// [`Self::merge_reach`] handing `f` a [`FoldedPlacement`] over the destination operand's own
    /// handle instead of a bare [`FoldToken`] — the bundle-threading twin of
    /// [`Witnessed::merge_pinned_placing`].
    pub fn merge_reach_placing<B, P, Pr, Pin>(
        self,
        self_bundle: &PinBundle<F>,
        other: Witnessed<B, Carrier<F>>,
        other_bundle: &PinBundle<F>,
        pin: &Pin,
        f: impl for<'b> FnOnce(T::At<'b>, B::At<'b>, FoldedPlacement<'b, Pr>) -> P::At<'b>,
    ) -> (Witnessed<P, Carrier<F>>, PinBundle<F>)
    where
        B: Reattachable,
        P: Reattachable,
        Pin: Witness,
        Pr: StorageProfile<FrameOwner = F> + 'static,
        F: RegionOwner<Region = Region<Pr>>,
        for<'b> B::At<'b>: super::HasRegionHandle<'b, Pr>,
    {
        self.merge_composed(
            other,
            pin,
            |left, right, live_dest| {
                Carrier::compose_into(
                    left,
                    right,
                    self_bundle,
                    other_bundle,
                    live_dest.region_handle(),
                    None,
                    Residence::Copied,
                )
            },
            super::place_over_dest::<T, B, P, Pr>(f),
        )
    }
}

/// The materialization rule (witness-hosting.md § Composition, rule 2, plus the `Kept` residence
/// pin): a `Kept` re-home always materializes the host — the value still lives there; a `Copied`
/// re-home materializes it only when the value's borrows genuinely reach it (`borrows_host`);
/// a `Released` re-home never materializes it — the embedder has already verified no borrow
/// survives into it, regardless of `borrows_host`.
fn materialize_hosts<F>(host: Option<&Rc<F>>, mode: Residence, borrows_host: bool) -> Vec<Rc<F>> {
    match (mode, host) {
        (Residence::Kept, Some(h)) => vec![Rc::clone(h)],
        (Residence::Copied, Some(h)) if borrows_host => vec![Rc::clone(h)],
        // `Released` never materializes the host, regardless of `borrows_host` — every other case
        // (no host, or `Copied`/`Kept` combinations already handled above) is likewise empty.
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use super::*;

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

    #[test]
    fn default_is_empty() {
        let c: Carrier<TestFrame> = Carrier::default();
        assert!(c.is_empty());
        assert!(!c.borrows_host());
    }

    #[test]
    fn clone_is_a_bit_copy_with_no_refcount_traffic() {
        let host = root_frame();
        let foreign = root_frame();
        let handle = RegionHandle::from_owner(&*host);
        let (minted, _bundle) =
            ReachDescription::mint(handle, &[], &[Rc::clone(&foreign)], |_| false);
        let c: Carrier<TestFrame> = Carrier::new(false, minted);
        let host_before = Rc::strong_count(&host);
        let foreign_before = Rc::strong_count(&foreign);
        let cloned = c;
        assert_eq!(Rc::strong_count(&host), host_before);
        assert_eq!(Rc::strong_count(&foreign), foreign_before);
        let _ = cloned;
        assert_eq!(Rc::strong_count(&foreign), foreign_before);
    }

    #[test]
    fn pinned_read_names_foreign_members_only() {
        let foreign = root_frame();
        let host = root_frame();
        // Mint a set naming `foreign` into `host`'s own region — the shape a resident bind produces.
        let handle = RegionHandle::from_owner(&*host);
        let (minted, _bundle) =
            ReachDescription::mint(handle, &[], &[Rc::clone(&foreign)], |_| false);
        let c: Carrier<TestFrame> = Carrier::new(false, minted);
        assert!(c.reach_covers(Some(&host), foreign.region()));
        assert!(
            !c.reach_covers(Some(&host), host.region()),
            "home is residence, not reach, without borrows_host"
        );
    }

    #[test]
    fn empty_carrier_reaches_nothing() {
        let frame = root_frame();
        let c: Carrier<TestFrame> = Carrier::default();
        assert!(!c.reach_covers(None, frame.region()));
        c.with_reach(None, |reach| assert!(reach.is_none()));
    }

    #[test]
    fn mint_kept_materializes_host_unconditionally() {
        let host = root_frame();
        let dest = root_frame();
        let c: Carrier<TestFrame> = Carrier::default();
        let handle = RegionHandle::from_owner(&*dest);
        let (minted, _bundle, borrows_into_dest) = c.mint_into(
            &PinBundle::empty(),
            handle,
            Some(&host),
            Residence::Kept,
            |_| false,
        );
        let set = minted.expect("Kept materializes the residence host");
        assert!(set.pins_region(host.region()));
        assert!(!borrows_into_dest);
    }

    #[test]
    fn mint_copied_drops_residence_only_host() {
        let host = root_frame();
        let dest = root_frame();
        let c: Carrier<TestFrame> = Carrier::default();
        let handle = RegionHandle::from_owner(&*dest);
        let (minted, _bundle, borrows_into_dest) = c.mint_into(
            &PinBundle::empty(),
            handle,
            Some(&host),
            Residence::Copied,
            |_| false,
        );
        assert!(
            minted.is_none(),
            "a residence-only host never rides a copied re-home"
        );
        assert!(!borrows_into_dest);
    }

    #[test]
    fn mint_copied_keeps_borrowing_host() {
        let host = root_frame();
        let dest = root_frame();
        let c: Carrier<TestFrame> = Carrier::new(true, None);
        let handle = RegionHandle::from_owner(&*dest);
        let (minted, _bundle, _) = c.mint_into(
            &PinBundle::empty(),
            handle,
            Some(&host),
            Residence::Copied,
            |_| false,
        );
        let set = minted.expect("a borrows_host value keeps its old home as a member");
        assert!(set.pins_region(host.region()));
    }

    #[test]
    fn mint_released_never_materializes_host_even_when_borrows_host_is_set() {
        let host = root_frame();
        let dest = root_frame();
        // `borrows_host: true` would keep the host alive under `Copied`
        // (`mint_copied_keeps_borrowing_host`); `Released` asserts the copy pass already verified
        // no surviving borrow, so the host never materializes regardless.
        let c: Carrier<TestFrame> = Carrier::new(true, None);
        let handle = RegionHandle::from_owner(&*dest);
        let (minted, _bundle, borrows_into_dest) = c.mint_into(
            &PinBundle::empty(),
            handle,
            Some(&host),
            Residence::Released,
            |_| false,
        );
        assert!(
            minted.is_none(),
            "Released never materializes the host, even when borrows_host is set"
        );
        assert!(
            !borrows_into_dest,
            "Released drops the host-pin term from borrows_into_dest"
        );
    }

    #[test]
    fn compose_released_drops_the_left_host_pin_term() {
        let host = root_frame();
        let dest = root_frame();
        let left: Carrier<TestFrame> = Carrier::new(true, None);
        let right: Carrier<TestFrame> = Carrier::default();
        let handle = RegionHandle::from_owner(&*dest);
        let (composed, _bundle) = Carrier::compose_into(
            &left,
            &right,
            &PinBundle::empty(),
            &PinBundle::empty(),
            handle,
            Some(&host),
            Residence::Released,
        );
        assert!(
            !composed.borrows_host(),
            "left's host-pin term is dropped from the composed bit under Released"
        );
    }

    #[test]
    fn mint_reports_borrows_into_dest_via_host_subsumption() {
        let dest = root_frame();
        let c: Carrier<TestFrame> = Carrier::new(true, None);
        let handle = RegionHandle::from_owner(&*dest);
        // The value's home IS the destination (host pins dest's region): borrows_host carries over.
        let (_, _bundle, borrows_into_dest) = c.mint_into(
            &PinBundle::empty(),
            handle,
            Some(&dest),
            Residence::Kept,
            |_| false,
        );
        assert!(borrows_into_dest);
    }

    #[test]
    fn mint_forwards_reach_members() {
        let foreign = root_frame();
        let host = root_frame();
        let dest = root_frame();
        let host_handle = RegionHandle::from_owner(&*host);
        let (source, bundle) =
            ReachDescription::mint(host_handle, &[], &[Rc::clone(&foreign)], |_| false);
        let c: Carrier<TestFrame> = Carrier::new(false, source);
        let dest_handle = RegionHandle::from_owner(&*dest);
        let (minted, _bundle, _) =
            c.mint_into(&bundle, dest_handle, Some(&host), Residence::Copied, |_| {
                false
            });
        let set = minted.expect("reach members always mint forward");
        assert!(set.pins_region(foreign.region()));
        assert!(
            !set.pins_region(host.region()),
            "residence-only host dropped on the copied direction"
        );
    }
}
