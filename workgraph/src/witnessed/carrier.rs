//! [`Carrier<F, S>`] — the collapsed carrier witness for a walking (in-flight, node-slot) value:
//! exactly one owned liveness arm plus a *reference* to a hosted reach set, replacing an owned
//! `{ pins: Vec<_>, reach: FrameSet }` pair. See
//! [design/witness-hosting.md § The carrier](../../../design/witness-hosting.md#the-carrier).
//!
//! A frame-backed clone is a bit-copy, one refcount bump (`host`), and a reference-copy (`reach`) —
//! no set allocation, no per-member refcount traffic. The set a `Hosted` carrier references is never
//! owned by it: [`RegionSet::mint`] is the only way such a set comes to exist, and it always lands in
//! *some* region's own arena — `Hosted.host` is exactly the arm that arena's owner backs, so holding
//! `host` pins the arena, hence the set, hence (through its members) every region the value reaches.
//! Never construct a `Hosted { reach: Some(_), .. }` whose set lives in a *different* region's arena
//! than `host`'s own — nothing would cover it.

use std::rc::Rc;

use super::{
    with_branded_ref, ComposeWitness, Erased, PinsRegion, Reattachable, Region, RegionHandle,
    RegionOwner, RegionSet, SetWitness, StorageProfile, Stored, Witness,
};

/// [`Reattachable`] family for a lifetime-erased `&RegionSet<F>` — the erased reach reference a
/// `Carrier::Hosted` re-anchors under its held `host` pin. Not exported: the pinned reader on
/// [`Carrier`] is the sole re-anchor site, so no branded reader escapes this module.
pub struct HostedSetRef<F>(std::marker::PhantomData<F>);

// SAFETY: `&'r RegionSet<F>` is a thin pointer whose layout does not depend on `'r`.
unsafe impl<F: PinsRegion + 'static> Reattachable for HostedSetRef<F> {
    type At<'r> = &'r RegionSet<F>;
}

/// A destination family's live form that exposes the [`RegionHandle`] mint target a
/// [`ComposeWitness`] impl needs to allocate a hosted set into — the compose-time counterpart of
/// [`super::WitnessRegion::region`] for a region/builder family's *live* form rather than its
/// witness. Each destination family a relocation/merge site uses (a region-ref family, an
/// aggregate-builder family) implements this once for its `At<'b>`.
///
/// # Safety
///
/// The returned handle must authorize allocation into the region `dest`'s own witness ultimately
/// pins, so a set minted through it is co-located with the composed carrier's `host`.
pub unsafe trait HasRegionHandle<'b, P: StorageProfile> {
    /// The region handle `dest` (or the region it builds into) allocates through.
    fn region_handle(&self) -> RegionHandle<'b, P>;
}

/// The collapsed walking-carrier witness: exactly one owned liveness arm, plus (for a hosted value)
/// a reference to a reach set living in that arm's own arena. `F` is the workload's frame-owner type
/// (`Rc<F>` is the residence pin); `S` is the workload's severed-backing type (a frame-free owned
/// node, transitional debt — see [design/witness-hosting.md § The carrier](
/// ../../../design/witness-hosting.md#the-carrier)).
#[derive(Default)]
pub enum Carrier<F: PinsRegion + 'static, S> {
    /// Frameless / run-region terminal: pins nothing, reaches nowhere. Backed by storage that
    /// outlives the carrier, so no held pin is required. The [`Default`].
    #[default]
    Empty,
    /// Frame-backed: the value lives in `host`'s region. `reach` is a reference into `host`'s OWN
    /// arena naming the value's foreign reach (home-omitted, so it never names `host`'s own region);
    /// `borrows_host` carries the separate "borrows into its own home" bit. Clone is a bit-copy plus
    /// one refcount bump (`host`) plus a reference-copy (`reach`) — no set allocation.
    Hosted {
        /// Whether the value's borrows also reach into `host`'s own region (materialized separately
        /// from `reach`, which is home-omitted by construction).
        borrows_host: bool,
        /// The residence pin: holding it keeps `host`'s arena — hence `reach`'s pointee, hence every
        /// region `reach` names — alive.
        host: Rc<F>,
        /// The value's foreign reach, erased and re-anchored only under the held `host` pin.
        /// `None` == the empty set (encoded without an allocation).
        reach: Option<Erased<HostedSetRef<F>>>,
    },
    /// Severed: the finalize sever's frame-free owned backing. No live host arena exists to host a
    /// set for such a value, so it OWNS its reach outright. Transitional debt, deleted along with
    /// the sever gate once the scheduler retains producer frames itself.
    Severed {
        /// The value's own owned backing (an `Rc`-shaped node) — holding it is what keeps the
        /// severed value's pointee alive.
        node: S,
        /// The value's foreign reach, owned (no host arena to reference into).
        reach: RegionSet<F>,
    },
}

impl<F: PinsRegion + 'static, S: Clone> Clone for Carrier<F, S> {
    fn clone(&self) -> Self {
        match self {
            Carrier::Empty => Carrier::Empty,
            Carrier::Hosted {
                borrows_host,
                host,
                reach,
            } => Carrier::Hosted {
                borrows_host: *borrows_host,
                host: Rc::clone(host),
                reach: *reach,
            },
            Carrier::Severed { node, reach } => Carrier::Severed {
                node: node.clone(),
                reach: reach.clone(),
            },
        }
    }
}

// SAFETY: `Empty` pins nothing — its backing outlives the carrier (the `Witness` contract's escape
// hatch for a frameless / run-region terminal). `Hosted.host` is a `StableDeref` `Rc<F>` (per the
// blanket `Witness for Rc<F>` impl), so holding it keeps `F`'s own storage alive. `Hosted.reach`'s
// pointee lives in an arena `host` keeps alive — either `host`'s own arena (every direct construction
// site), or an arena rooted by a set that `host`'s own arena holds (the Borrowed-window read, where
// the USING overlay's construction minted the module region into the reader's arena before any such
// carrier exists) — so holding `host` roots `reach`'s pointee either directly or one hop removed, and
// through its members every region it names, alive. `Severed.node` is the value's own owned backing
// (an `Rc`-shaped `S`, by the sole production constructor's obligation), so holding it keeps the
// severed value's pointee alive the same way a `Hosted.host` does; `Severed.reach` is an owned
// `RegionSet`, independently `Witness`-sound.
unsafe impl<F: PinsRegion + 'static, S> Witness for Carrier<F, S> {}

// SAFETY: `singleton(single)` holds `single` as the sole `host` arm with an empty reach, so the
// `Witness` impl above pins exactly `single`'s region for as long as the carrier is held — the
// single-region `yoke` precondition.
unsafe impl<F: PinsRegion + 'static, S> SetWitness<Rc<F>> for Carrier<F, S> {
    fn singleton(single: Rc<F>) -> Self {
        Carrier::Hosted {
            borrows_host: false,
            host: single,
            reach: None,
        }
    }
}

impl<F: PinsRegion + 'static, S> Carrier<F, S> {
    /// Whether this carrier pins and reaches nothing — the frameless / run-region terminal, whose
    /// backing outlives the carrier so no re-home is needed.
    pub fn is_empty(&self) -> bool {
        matches!(self, Carrier::Empty)
    }

    /// Read the reach set this carrier references (or owns), pinned under whatever this carrier
    /// itself holds — the sole re-anchor of a `Hosted.reach` erased reference. `None` when the
    /// carrier reaches nothing (`Empty`, or a `Hosted` with an empty reach). Public: a caller
    /// re-hosting this carrier's value into its own arena under a policy the generic
    /// [`ComposeWitness`] composition doesn't cover (an extra omission predicate beyond the mint's
    /// built-in self-cycle rule) routes a direct [`RegionSet::mint`] through this reader instead.
    pub fn with_reach<R>(&self, f: impl FnOnce(Option<&RegionSet<F>>) -> R) -> R {
        match self {
            Carrier::Empty => f(None),
            Carrier::Hosted { reach: None, .. } => f(None),
            Carrier::Hosted {
                reach: Some(erased),
                ..
            } => with_branded_ref::<HostedSetRef<F>, R>(erased.as_static(), |set_ref: &&_| {
                f(Some(*set_ref))
            }),
            Carrier::Severed { reach, .. } => f(Some(reach)),
        }
    }

    /// Whether the value's **reach** already names `region` — reach members, plus `host`'s own
    /// region when `borrows_host` is set. The finalize gate and the bind-bit derivation key on this.
    pub fn reach_covers(&self, region: &F::Region) -> bool {
        match self {
            Carrier::Empty => false,
            Carrier::Hosted {
                borrows_host, host, ..
            } => {
                (*borrows_host && host.pins_region(region))
                    || self.with_reach(|reach| reach.is_some_and(|r| r.pins_region(region)))
            }
            Carrier::Severed { reach, .. } => reach.pins_region(region),
        }
    }

    /// Whether **anything** this carrier holds — the host residence pin or a reach member — keeps
    /// `region` alive. The step-liveness query, where residence counts just as much as reach.
    pub fn covers(&self, region: &F::Region) -> bool {
        match self {
            Carrier::Empty => false,
            Carrier::Hosted { host, .. } => {
                host.pins_region(region)
                    || self.with_reach(|reach| reach.is_some_and(|r| r.pins_region(region)))
            }
            Carrier::Severed { reach, .. } => reach.pins_region(region),
        }
    }

    /// The value's owned foreign reach: a clone of `Hosted.reach`'s content (never `host` itself,
    /// which this deliberately excludes) or `Severed.reach` verbatim; empty for `Empty`. The
    /// finalize sever's building block — it needs an owned clone of the reach *before* the producer
    /// host it was read out of is dropped.
    pub fn to_owned_reach(&self) -> RegionSet<F> {
        match self {
            Carrier::Empty => RegionSet::empty(),
            Carrier::Hosted { .. } => self.with_reach(|reach| reach.cloned().unwrap_or_default()),
            Carrier::Severed { reach, .. } => reach.clone(),
        }
    }

    /// Collapse to a plain [`RegionSet`] naming every region this carrier keeps alive through a
    /// frame (`host` ∪ reach members for `Hosted`; the owned reach for `Severed`; empty for
    /// `Empty`) — the step-open liveness pin a consumer folds when it has no arena to host-mint
    /// against. A `Severed.node` carries no region and is deliberately not representable here.
    pub fn to_liveness_frameset(&self) -> RegionSet<F> {
        match self {
            Carrier::Empty => RegionSet::empty(),
            Carrier::Hosted { host, .. } => {
                let mut set = RegionSet::singleton(Rc::clone(host));
                self.with_reach(|reach| {
                    if let Some(reach) = reach {
                        set = RegionSet::union(&set, reach);
                    }
                });
                set
            }
            Carrier::Severed { reach, .. } => reach.clone(),
        }
    }
}

// SAFETY: identical obligation to `UnionWitness::union`, discharged by minting instead of by owned
// union. `right` names the destination's own identity witness — `Empty` for a pure-data merge peer
// (nothing to add; `left` already pins everything), or `Hosted` for a genuine relocation (`left`'s
// reach is minted into `dest`'s own arena and the result re-homes onto `right`'s host, demoting
// `left`'s own host, if any, to an ordinary reach *member* rather than dropping it, so its `Rc` is
// still cloned forward — so holding the composed carrier keeps `dest`'s arena — hence the minted
// set, hence every region `left` reached — alive, exactly like `left` did before relocation).
// `right` is never `Severed`: a severed backing hosts no arena to relocate into, so it never arises
// as a merge/transfer_into destination.
//
// A `left` that is itself `Severed` has no slot in the composed carrier for its owned `node` (only
// its `reach` is minted forward), which leaves two obligations at the merge boundary — both
// deleted with the `Severed` arm (`delivery-driven-frame-retention`).
//
// Result validity: a projection may pass `left`'s value through un-copied only when `left` is
// provably never `Severed`, or when the passed-through view is never read; every other projection
// must copy (`deep_clone` / `copy_carried` / `Held::from_carried`). Two non-copying discharges
// exist today: the `alloc_type_with` / `alloc_object_with` veneers (`src/machine/core/arena.rs`)
// fold dep views via `fold_dep_view` but discard them, never reading the pushed view; and the one
// view-reading fold (`src/machine/model/values/attr.rs`, the field lookup) consumes a dep built by
// `resident_value_carrier`, which is always `Hosted`, never `Severed`.
//
// Deallocation timing: `Witnessed::merge` drops the source witnesses before it returns, while the
// call-duration protectors on its by-value operands' interior references are still active — so the
// `node` must not hit refcount zero inside the call even when the projection copied the value out.
// A consuming-merge caller that can present a `Severed` left holds a node pin across the call (the
// severed-backing pin in `finalize_terminal`, `src/machine/execute/finalize.rs`);
// `Sealed::transfer_into` discharges it structurally — it merges a duplicate while the borrowed
// seal retains the original witness past the call.
unsafe impl<F, S, P, B> ComposeWitness<B> for Carrier<F, S>
where
    F: PinsRegion + RegionOwner<Region = Region<P>> + 'static,
    S: Clone,
    P: StorageProfile,
    B: Reattachable,
    for<'b> B::At<'b>: HasRegionHandle<'b, P>,
    RegionSet<F>: Stored<P> + for<'r> Reattachable<At<'r> = RegionSet<F>>,
    P: 'static,
{
    fn compose<'b>(left: &Self, right: &Self, dest: &B::At<'b>) -> Self {
        match right {
            Carrier::Empty => left.clone(),
            Carrier::Hosted {
                host,
                borrows_host: right_borrows_host,
                ..
            } => {
                let handle = dest.region_handle();
                let dest_host = Rc::clone(host);
                let new_borrows_host = *right_borrows_host || left.reach_covers(dest_host.region());
                let materialize: Vec<Rc<F>> = match left {
                    Carrier::Hosted { host: lh, .. } => vec![Rc::clone(lh)],
                    _ => Vec::new(),
                };
                // Mint BOTH operands' exact reach — `right`'s own reach (an accumulator's prior
                // folds, already minted into this same `dest` arena, so re-minting is idempotent
                // via subsumption) and `left`'s (the newly-folded source) — never `left` alone, or
                // a multi-step accumulator fold would drop everything folded before this step.
                let minted = left.with_reach(|left_reach| {
                    right.with_reach(|right_reach| {
                        let sources: Vec<&RegionSet<F>> =
                            left_reach.into_iter().chain(right_reach).collect();
                        RegionSet::mint(handle, &sources, &materialize, |_| false)
                    })
                });
                Carrier::Hosted {
                    borrows_host: new_borrows_host,
                    host: dest_host,
                    reach: minted.map(Erased::<HostedSetRef<F>>::erase),
                }
            }
            Carrier::Severed { .. } => unreachable!(
                "ComposeWitness destination carrier is never itself Severed — a severed backing \
                 hosts no arena to relocate/merge into"
            ),
        }
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
        let c: Carrier<TestFrame, ()> = Carrier::default();
        assert!(c.is_empty());
    }

    #[test]
    fn singleton_is_hosted_with_no_reach() {
        let frame = root_frame();
        let c: Carrier<TestFrame, ()> = Carrier::singleton(Rc::clone(&frame));
        assert!(!c.is_empty());
        assert!(c.covers(frame.region()));
        assert!(
            !c.reach_covers(frame.region()),
            "singleton carries no reach — only residence"
        );
    }

    #[test]
    fn cheap_clone_bumps_host_refcount_only() {
        let frame = root_frame();
        let c: Carrier<TestFrame, ()> = Carrier::singleton(Rc::clone(&frame));
        let before = Rc::strong_count(&frame);
        let cloned = c.clone();
        assert_eq!(Rc::strong_count(&frame), before + 1);
        drop(cloned);
        assert_eq!(Rc::strong_count(&frame), before);
    }

    #[test]
    fn pinned_read_returns_host_and_foreign_members() {
        let foreign = root_frame();
        let host = root_frame();
        // Mint a set naming `foreign` into `host`'s own region — the shape a resident bind produces.
        let handle = RegionHandle::from_owner(&*host);
        let minted: Option<&RegionSet<TestFrame>> =
            RegionSet::mint(handle, &[], &[Rc::clone(&foreign)], |_| false);
        let reach = minted.map(Erased::<HostedSetRef<TestFrame>>::erase);
        let c: Carrier<TestFrame, ()> = Carrier::Hosted {
            borrows_host: false,
            host: Rc::clone(&host),
            reach,
        };
        assert!(c.covers(host.region()));
        assert!(c.covers(foreign.region()));
        assert!(c.reach_covers(foreign.region()));
        assert!(
            !c.reach_covers(host.region()),
            "host is residence, not reach, without borrows_host"
        );
    }

    #[test]
    fn to_liveness_frameset_unions_host_and_reach() {
        let foreign = root_frame();
        let host = root_frame();
        let handle = RegionHandle::from_owner(&*host);
        let minted: Option<&RegionSet<TestFrame>> =
            RegionSet::mint(handle, &[], &[Rc::clone(&foreign)], |_| false);
        let reach = minted.map(Erased::<HostedSetRef<TestFrame>>::erase);
        let c: Carrier<TestFrame, ()> = Carrier::Hosted {
            borrows_host: false,
            host: Rc::clone(&host),
            reach,
        };
        let liveness = c.to_liveness_frameset();
        assert!(liveness.pins_region(host.region()));
        assert!(liveness.pins_region(foreign.region()));
    }

    #[test]
    fn empty_carrier_reaches_and_covers_nothing() {
        let frame = root_frame();
        let c: Carrier<TestFrame, ()> = Carrier::Empty;
        assert!(!c.covers(frame.region()));
        assert!(!c.reach_covers(frame.region()));
        assert!(c.to_liveness_frameset().is_empty());
    }
}
