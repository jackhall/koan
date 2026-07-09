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
//! never owned by it: [`RegionSet::mint`] is the only way such a set comes to exist, and it always
//! lands in the value's host region's own arena, so whatever covers the host arena covers the set
//! and, through its members, every region the value reaches.
//!
//! `Carrier` is deliberately **not** a [`super::Witness`]: a bare [`super::Sealed::open`] under it
//! does not compile. Reads name their coverage — [`super::Sealed::open_with`] under an external
//! pin, or the envelope's [`Delivered::open`](super::Delivered::open) — and relocations run
//! through the envelope-bearing mint verbs ([`Delivered::mint_reach`](super::Delivered::mint_reach),
//! [`Delivered::transfer_into`](super::Delivered::transfer_into)), the only places a residence
//! host materializes as a set member.

use std::rc::Rc;

use super::{
    with_branded_ref, ComposeWitness, Erased, PinsRegion, Reattachable, Region, RegionHandle,
    RegionOwner, RegionSet, StorageProfile, Stored,
};

/// [`Reattachable`] family for a lifetime-erased `&RegionSet<F>` — the erased reach reference a
/// [`Carrier`] re-anchors under an externally supplied pin. Module-private: the pinned reader on
/// [`Carrier`] is the sole re-anchor site, so no branded reader escapes this module.
struct HostedSetRef<F>(std::marker::PhantomData<F>);

// SAFETY: `&'r RegionSet<F>` is a thin pointer whose layout does not depend on `'r`.
unsafe impl<F: PinsRegion + 'static> Reattachable for HostedSetRef<F> {
    type At<'r> = &'r RegionSet<F>;
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
    pub fn new(borrows_host: bool, reach: Option<&RegionSet<F>>) -> Self {
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
    /// `Scheduler::payload_of`'s gate) rather than split into a `pub(crate)` core.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn borrows_host(&self) -> bool {
        self.borrows_host
    }

    /// Whether this carrier represents no reach at all — the frameless / region-pure terminal,
    /// whose backing outlives the carrier so no pin or mint is ever needed for it.
    pub fn is_empty(&self) -> bool {
        !self.borrows_host && self.reach.is_none()
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
    fn with_reach_impl<R>(
        &self,
        pin: Option<&Rc<F>>,
        f: impl FnOnce(Option<&RegionSet<F>>) -> R,
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

    /// White-box reach introspection: kept ungated `pub(crate)` for [`Self::mint_into`] /
    /// [`Self::compose_into`]'s own use, and re-exposed `pub` only under `test-hooks` for an
    /// embedder's white-box tests (mirroring `Scheduler::payload_of`'s gate).
    #[cfg(not(any(test, feature = "test-hooks")))]
    pub(crate) fn with_reach<R>(
        &self,
        pin: Option<&Rc<F>>,
        f: impl FnOnce(Option<&RegionSet<F>>) -> R,
    ) -> R {
        self.with_reach_impl(pin, f)
    }

    /// See [`Self::with_reach_impl`].
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn with_reach<R>(
        &self,
        pin: Option<&Rc<F>>,
        f: impl FnOnce(Option<&RegionSet<F>>) -> R,
    ) -> R {
        self.with_reach_impl(pin, f)
    }

    /// Whether the value's foreign reach names `region` — reach members only; the borrows-into-home
    /// bit is a separate query ([`Self::borrows_host`]) because the home it refers to is the
    /// envelope's knowledge, not the carrier's. `pin` covers the reach set's hosting arena, as in
    /// [`Self::with_reach`].
    ///
    /// White-box reach introspection: same `test-hooks` gate as [`Self::with_reach`] /
    /// [`Self::borrows_host`].
    #[cfg(not(any(test, feature = "test-hooks")))]
    pub(crate) fn reach_covers(&self, pin: Option<&Rc<F>>, region: &F::Region) -> bool {
        self.with_reach_impl(pin, |reach| reach.is_some_and(|r| r.pins_region(region)))
    }

    /// See [`Self::reach_covers`] above.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn reach_covers(&self, pin: Option<&Rc<F>>, region: &F::Region) -> bool {
        self.with_reach_impl(pin, |reach| reach.is_some_and(|r| r.pins_region(region)))
    }

    /// Mint this carrier's reach into `dest`, materializing `host` (the value's producer frame
    /// owner) per `mode`, and report whether the value's borrows reach `dest`'s own region — the
    /// re-home mint every bind / adoption routes (through the envelope's
    /// [`mint_reach`](super::Delivered::mint_reach), or directly with an `Option` host at a site
    /// whose value is resident or whose producer is frameless). Applies, via [`RegionSet::mint`]:
    /// home-omission (`dest`'s own region is never a member), the caller's `omit` policy
    /// predicate, and outer-chain subsumption.
    ///
    /// `host` doubles as the pin for the source-set read (`with_reach`); `None` asserts
    /// the source arena is ambiently covered (a resident value's own region, a held step pin) —
    /// and also that there is no residence to materialize, so `mode` only gates a `Some` host.
    /// Returns the minted set (`None` == empty, no allocation) and the borrows-into-dest bit:
    /// reach members pinning `dest`'s region, or the `borrows_host` bit when `host` itself pins it
    /// (the value's home *is* — or subsumes — the destination).
    pub fn mint_into<'d, P>(
        &self,
        dest: RegionHandle<'d, P>,
        host: Option<&Rc<F>>,
        mode: Residence,
        omit: impl Fn(&Region<P>) -> bool,
    ) -> (Option<&'d RegionSet<F>>, bool)
    where
        P: StorageProfile + 'static,
        F: RegionOwner<Region = Region<P>>,
        RegionSet<F>: Stored<P> + for<'r> Reattachable<At<'r> = RegionSet<F>>,
    {
        let materialize = materialize_hosts(host, mode, self.borrows_host);
        let minted = self.with_reach(host, |reach| {
            let sources: &[&RegionSet<F>] = match &reach {
                Some(r) => std::slice::from_ref(r),
                None => &[],
            };
            RegionSet::mint(dest, sources, &materialize, omit)
        });
        let borrows_into_dest = self.reach_covers(host, dest.region())
            || (self.borrows_host && host.is_some_and(|h| h.pins_region(dest.region())));
        (minted, borrows_into_dest)
    }

    /// The relocation composition behind the envelope's
    /// [`transfer_into`](super::Delivered::transfer_into) and the generic [`ComposeWitness`]
    /// impl: mint BOTH operands' exact reach — `right`'s (an accumulator's prior folds, already
    /// minted into this same `dest` arena, so re-minting is idempotent via subsumption) and
    /// `left`'s (the newly-folded source) — plus `left`'s `host` per `mode`, into `dest`'s arena,
    /// and compute the composed borrows-into-dest bit. Never `left` alone, or a multi-step
    /// accumulator fold would drop everything folded before this step.
    pub(in crate::witnessed) fn compose_into<'b, P>(
        left: &Self,
        right: &Self,
        dest: RegionHandle<'b, P>,
        host: Option<&Rc<F>>,
        mode: Residence,
    ) -> Self
    where
        P: StorageProfile + 'static,
        F: RegionOwner<Region = Region<P>>,
        RegionSet<F>: Stored<P> + for<'r> Reattachable<At<'r> = RegionSet<F>>,
    {
        let materialize = materialize_hosts(host, mode, left.borrows_host);
        // `left`'s reach reads under the supplied host pin; `right`'s reach is the destination's
        // own prior folds, hosted in `dest`'s arena — covered by the live `dest` the caller is
        // composing against.
        let minted = left.with_reach(host, |left_reach| {
            right.with_reach(None, |right_reach| {
                let sources: Vec<&RegionSet<F>> =
                    left_reach.into_iter().chain(right_reach).collect();
                RegionSet::mint(dest, &sources, &materialize, |_| false)
            })
        });
        let borrows_into_dest = right.borrows_host
            || left.reach_covers(host, dest.region())
            || (left.borrows_host && host.is_some_and(|h| h.pins_region(dest.region())));
        Carrier {
            borrows_host: borrows_into_dest,
            reach: minted.map(Erased::<HostedSetRef<F>>::erase),
        }
    }
}

/// The materialization rule (witness-hosting.md § Composition, rule 2, plus the `Kept` residence
/// pin): a `Kept` re-home always materializes the host — the value still lives there; a `Copied`
/// re-home materializes it only when the value's borrows genuinely reach it (`borrows_host`).
fn materialize_hosts<F>(host: Option<&Rc<F>>, mode: Residence, borrows_host: bool) -> Vec<Rc<F>> {
    match (mode, host) {
        (Residence::Kept, Some(h)) => vec![Rc::clone(h)],
        (Residence::Copied, Some(h)) if borrows_host => vec![Rc::clone(h)],
        _ => Vec::new(),
    }
}

// SAFETY: `ComposeWitness`'s obligation is representational for a reference-only witness: the
// composed carrier must NAME every region the relocated value's borrows reach, relative to `dest`.
// `compose_into` mints both operands' exact reach into `dest`'s own arena (so the composed
// reference is covered by whatever covers `dest`) and derives the borrows-into-dest bit from both
// operands' bits and members. With no host in hand there is nothing to materialize — this is the
// PURE reach mint; a relocation whose source has a residence pin to fold routes the
// envelope-bearing [`Delivered::transfer_into`](super::Delivered::transfer_into) instead, which
// supplies the host. Both operands' backings are externally covered across the composition (the
// pinned-merge caller's contract).
unsafe impl<F, P, B> ComposeWitness<B> for Carrier<F>
where
    F: PinsRegion + RegionOwner<Region = Region<P>> + 'static,
    P: StorageProfile,
    B: Reattachable,
    for<'b> B::At<'b>: HasRegionHandle<'b, P>,
    RegionSet<F>: Stored<P> + for<'r> Reattachable<At<'r> = RegionSet<F>>,
    P: 'static,
{
    fn compose<'b>(left: &Self, right: &Self, dest: &B::At<'b>) -> Self {
        Self::compose_into(left, right, dest.region_handle(), None, Residence::Copied)
    }
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use super::*;
    use crate::witnessed::{FamilyArena, StorageOf};

    struct TestProfile;

    impl StorageProfile for TestProfile {
        type Families = (RegionSet<TestFrame>, ());
    }

    impl Stored<TestProfile> for RegionSet<TestFrame> {
        fn cell(storage: &StorageOf<TestProfile>) -> &FamilyArena<Self> {
            &storage.0
        }
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
        let minted: Option<&RegionSet<TestFrame>> =
            RegionSet::mint(handle, &[], &[Rc::clone(&foreign)], |_| false);
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
        let minted: Option<&RegionSet<TestFrame>> =
            RegionSet::mint(handle, &[], &[Rc::clone(&foreign)], |_| false);
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
        let (minted, borrows_into_dest) =
            c.mint_into(handle, Some(&host), Residence::Kept, |_| false);
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
        let (minted, borrows_into_dest) =
            c.mint_into(handle, Some(&host), Residence::Copied, |_| false);
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
        let (minted, _) = c.mint_into(handle, Some(&host), Residence::Copied, |_| false);
        let set = minted.expect("a borrows_host value keeps its old home as a member");
        assert!(set.pins_region(host.region()));
    }

    #[test]
    fn mint_reports_borrows_into_dest_via_host_subsumption() {
        let dest = root_frame();
        let c: Carrier<TestFrame> = Carrier::new(true, None);
        let handle = RegionHandle::from_owner(&*dest);
        // The value's home IS the destination (host pins dest's region): borrows_host carries over.
        let (_, borrows_into_dest) = c.mint_into(handle, Some(&dest), Residence::Kept, |_| false);
        assert!(borrows_into_dest);
    }

    #[test]
    fn mint_forwards_reach_members() {
        let foreign = root_frame();
        let host = root_frame();
        let dest = root_frame();
        let host_handle = RegionHandle::from_owner(&*host);
        let source: Option<&RegionSet<TestFrame>> =
            RegionSet::mint(host_handle, &[], &[Rc::clone(&foreign)], |_| false);
        let c: Carrier<TestFrame> = Carrier::new(false, source);
        let dest_handle = RegionHandle::from_owner(&*dest);
        let (minted, _) = c.mint_into(dest_handle, Some(&host), Residence::Copied, |_| false);
        let set = minted.expect("reach members always mint forward");
        assert!(set.pins_region(foreign.region()));
        assert!(
            !set.pins_region(host.region()),
            "residence-only host dropped on the copied direction"
        );
    }
}
