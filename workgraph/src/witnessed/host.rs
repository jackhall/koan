//! [`RegionHost`]: the library-owned region owner with a **lazily minted** region. A workload's
//! per-call frame storage is (or wraps) a `RegionHost<P>` — the region it names is created on first
//! [`region()`](RegionHost::region) access, not at construction, so a frame that never allocates
//! mints nothing. `outer` is the ancestor-frame link [`RegionHost::pins_region`] walks for
//! [`PinBundle`](super::PinBundle) subsumption; the same shape [`RegionOwner`] / [`PinsRegion`] are
//! implemented against everywhere in the workgraph model.

use std::cell::OnceCell;
use std::rc::{Rc, Weak};

#[cfg(any(test, feature = "test-hooks"))]
use std::cell::Cell;

#[cfg(debug_assertions)]
use std::cell::RefCell;

use super::{PinsRegion, Region, RegionOwner, StorageProfile};

/// A region owner whose region is minted on first use. Held behind an `Rc` at every call site: the
/// `outer` link is a strong pin on the ancestor's storage, so a chain of `RegionHost`s keeps every
/// ancestor region alive for as long as the deepest descendant is held.
pub struct RegionHost<P: StorageProfile> {
    /// Lazily minted on first [`region()`](Self::region) access — the library's mint point.
    /// Declared before `outer` so the region drops before the ancestor storage it may reference
    /// (field order is load-bearing, mirroring every `RegionHost`-shaped frame owner).
    region: OnceCell<Region<P>>,
    /// The parent's storage: both a liveness pin — held so the ancestor's storage outlives this
    /// host's own borrows into it — and the link [`Self::pins_region`] walks for subsumption. Drop
    /// tears down the chain in order.
    outer: Option<Rc<RegionHost<P>>>,
    /// The host's **tier**: `true` for an eternal host ([`Self::fresh_eternal`]), whose region
    /// outlives every region that could retain it; `false` for every per-call host
    /// ([`Self::fresh`]). Not derivable from the chain — a per-call host started at a fresh tail
    /// also has `outer: None` — so the owner carries the bit, and a workload asking "is this
    /// eternal?" reads it here rather than stamping a shadow copy on every structure the region
    /// backs.
    eternal: bool,
    /// This host's own `Weak`, captured at construction through `Rc::new_cyclic` — the back-link
    /// [`Self::region`] hands the region it mints, so every reach description frozen into that
    /// region's side table can stamp the owner it is hosted by ([`Region::host`]). A `Weak`, so it
    /// closes no cycle on the `Rc` that owns this host.
    me: Weak<P::FrameOwner>,
}

/// The constructors — available for a profile whose frame owner **is** this host type. A
/// `RegionHost<P>` mints `Region<P>`, whose descriptions are typed at `P::FrameOwner`, so the
/// back-link a fresh host captures for itself is only well-typed when the two coincide. Every
/// workload that owns its regions through `RegionHost` satisfies it by definition.
impl<P: StorageProfile<FrameOwner = RegionHost<P>>> RegionHost<P> {
    /// Build a fresh **per-call** host with no region minted yet, chained to `outer`.
    pub fn fresh(outer: Option<Rc<RegionHost<P>>>) -> Rc<Self> {
        Rc::new_cyclic(|me| RegionHost {
            region: OnceCell::new(),
            outer,
            eternal: false,
            me: me.clone(),
        })
    }

    /// Build an **eternal** host: no region minted yet, no ancestor, and marked at the eternal tier
    /// ([`Self::is_eternal`]) — its region outlives every region that could retain it, so nothing
    /// ever takes an owning pin on it. A workload's run root is one; so is storage a workload stands
    /// up ahead of the run and holds for the whole of it.
    pub fn fresh_eternal() -> Rc<Self> {
        Rc::new_cyclic(|me| RegionHost {
            region: OnceCell::new(),
            outer: None,
            eternal: true,
            me: me.clone(),
        })
    }
}

impl<P: StorageProfile> RegionHost<P> {
    /// Whether this host is at the eternal tier ([`Self::fresh_eternal`]) rather than a per-call
    /// frame. The tier a caller consults to decide whether chaining a strong pin to this storage is
    /// meaningful: an eternal region outlives everything that could retain it, so pinning it buys
    /// nothing and closes an `Rc` cycle.
    pub fn is_eternal(&self) -> bool {
        self.eternal
    }

    /// The backing region, minting it on first call. This is the **sole** mint point: nothing else
    /// in the library or a workload ever constructs a `Region<P>` directly against a `RegionHost`.
    /// The fresh region is handed this host's own `Weak`, so every description it later hosts names
    /// the owner it lives in.
    ///
    /// The `get_or_init` result is deliberately discarded and the reference re-derived through a
    /// plain `get`: the reference `get_or_init` returns on the minting call descends from the init
    /// frame's unique tag, which the next foreign handle's interior arena write would disable under
    /// tree borrows — poisoning everything stored through it. Re-deriving gives the minting caller
    /// the same shared-read lineage every later caller gets.
    pub fn region(&self) -> &Region<P> {
        let _ = self.region.get_or_init(|| {
            #[cfg(any(test, feature = "test-hooks"))]
            note_mint();
            Region::new(self.me.clone())
        });
        self.region.get().expect("initialized just above")
    }

    /// A non-minting peek at the region — `Some` iff [`region()`](Self::region) has already been
    /// called. Used by identity walks ([`Self::pins_region`]) that must not mint as a side effect of
    /// checking whether something is pinned.
    pub fn minted(&self) -> Option<&Region<P>> {
        self.region.get()
    }

    /// The parent host, if any.
    pub fn outer(&self) -> Option<&Rc<RegionHost<P>>> {
        self.outer.as_ref()
    }

    /// True iff holding `self`'s `Rc` keeps the region at `region` alive — `self`'s own (already
    /// minted) region or any of its `outer` ancestors' (each pinned by the chain). A host whose own
    /// region is not yet minted has nothing of its own to compare, so the walk simply continues to
    /// its ancestors.
    pub fn pins_region(&self, region: &Region<P>) -> bool {
        let mut node = self;
        loop {
            if let Some(minted) = node.minted()
                && std::ptr::eq(minted, region)
            {
                return true;
            }
            match &node.outer {
                Some(outer) => node = outer,
                None => return false,
            }
        }
    }
}

// SAFETY: a held `Rc<RegionHost<P>>` keeps its owned `RegionHost` — and the `Region<P>` field within
// it, along with the arena pages a value lives in — at a fixed heap address for the whole life of the
// `Rc` (`Rc` is `StableDeref`), so `region()` returns a reference into storage the `RegionOwner`
// blanket impl's `Rc<F>: WitnessRegion` pins: a value built solely from that region is pinned by
// holding the `Rc`. The `OnceCell` initializes in place inside the `Rc` box, so the region's address
// is fixed from mint to drop — the mint happening later than construction changes nothing about that
// address stability, only when it first exists.
unsafe impl<P: StorageProfile> RegionOwner for RegionHost<P> {
    type Region = Region<P>;
    fn region(&self) -> &Region<P> {
        RegionHost::region(self)
    }
}

// SAFETY: `pins_region` walks self's own (already-minted) region and its `outer` ancestor chain;
// holding self's `Rc` holds each ancestor `Rc` in turn, so every region the walk reports pinned stays
// live and fixed-address while self is held.
unsafe impl<P: StorageProfile> PinsRegion for RegionHost<P> {
    fn pins_region(&self, region: &Region<P>) -> bool {
        RegionHost::pins_region(self, region)
    }

    fn needs_no_pin(&self) -> bool {
        self.eternal
    }

    /// The chain [`Self::pins_region`] walks, reported as regions rather than as an answer to one
    /// query. A host whose own region is unminted contributes nothing — nothing can be retained in
    /// a region that does not exist — and `minted()` is what keeps the survey from minting one.
    #[cfg(debug_assertions)]
    fn for_each_pinned_region(&self, visit: &mut dyn FnMut(&Region<P>)) {
        let mut node = self;
        loop {
            if let Some(minted) = node.minted() {
                visit(minted);
            }
            match &node.outer {
                Some(outer) => node = outer,
                None => return,
            }
        }
    }
}

/// One detected **pin ring**: a chain of region owners along which liveness flows back to the region
/// whose retention closed it, so neither end can ever be freed. Recorded by the debug-mode detector
/// at [`Region::retain_reach`](super::Region::retain_reach) — the one moment both ends of the ring
/// are in hand.
///
/// Addresses are `Rc::as_ptr` owner identities rather than references: a report outlives the walk
/// that produced it, and a ring by definition holds its own members alive, so the identities stay
/// meaningful for as long as the leak they describe.
#[cfg(debug_assertions)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinCycleReport {
    /// The owner of the region whose retention closed the ring — where the blame lands.
    pub retainer: usize,
    /// The owner chain walked from the newly retained member (first) to the owner whose own pins
    /// reach back to the retaining region (last).
    pub path: Vec<usize>,
}

#[cfg(debug_assertions)]
thread_local! {
    static PIN_CYCLES: RefCell<Vec<PinCycleReport>> = const { RefCell::new(Vec::new()) };
}

/// Record one detected ring. Called from the detector alone.
#[cfg(debug_assertions)]
pub(super) fn note_pin_cycle(report: PinCycleReport) {
    PIN_CYCLES.with(|log| log.borrow_mut().push(report));
}

/// Every pin ring detected on this thread since the last reset, oldest first. Cloned out, so a
/// reader cannot narrow the log in place.
#[cfg(debug_assertions)]
pub fn pin_cycle_reports() -> Vec<PinCycleReport> {
    PIN_CYCLES.with(|log| log.borrow().clone())
}

/// Empty the ring log for this thread. Callers reset before a measured run so
/// [`pin_cycle_reports`] reads back that run's own detections only.
#[cfg(debug_assertions)]
pub fn reset_pin_cycle_reports() {
    PIN_CYCLES.with(|log| log.borrow_mut().clear());
}

/// Snapshot of the thread-local region counters — region mints, plus the reach side table's intern
/// and retention traffic. `peak` and every `_total`-shaped counter are monotonic across
/// [`reset_region_metrics`] calls only in the sense that a reset zeroes them; within one measurement
/// window they only grow, while `live` also falls as hosts drop.
///
/// The reach counters live here rather than on [`Region`] so a region grows no `cfg`-gated field:
/// they are per-thread totals across every region, which is what a test measuring one region's
/// traffic in isolation reads after a reset.
#[cfg(any(test, feature = "test-hooks"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RegionMetrics {
    /// Number of `RegionHost`s with a minted region that have not yet dropped.
    pub live: usize,
    /// High-water mark of `live` since the last reset.
    pub peak: usize,
    /// Total number of mints since the last reset (never decremented).
    pub minted_total: usize,
    /// Reach descriptions allocated — intern **misses** ([`Region::intern_reach_retained`]).
    pub reach_interned: usize,
    /// Intern **hits**: a mint that found its member set already described in the region.
    pub reach_intern_hits: usize,
    /// Folds into a region's union bundle ([`Region::retain_reach`]).
    pub reach_retention_folds: usize,
}

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    static LIVE: Cell<usize> = const { Cell::new(0) };
    static PEAK: Cell<usize> = const { Cell::new(0) };
    static MINTED_TOTAL: Cell<usize> = const { Cell::new(0) };
    static REACH_INTERNED: Cell<usize> = const { Cell::new(0) };
    static REACH_INTERN_HITS: Cell<usize> = const { Cell::new(0) };
    static REACH_RETENTION_FOLDS: Cell<usize> = const { Cell::new(0) };
}

/// Records an intern miss — one fresh description allocated in some region's side table.
#[cfg(any(test, feature = "test-hooks"))]
pub(super) fn note_reach_interned() {
    REACH_INTERNED.with(|c| c.set(c.get() + 1));
}

/// Records an intern hit — a mint that reused an existing description.
#[cfg(any(test, feature = "test-hooks"))]
pub(super) fn note_reach_intern_hit() {
    REACH_INTERN_HITS.with(|c| c.set(c.get() + 1));
}

/// Records a fold into some region's union bundle.
#[cfg(any(test, feature = "test-hooks"))]
pub(super) fn note_reach_retention_fold() {
    REACH_RETENTION_FOLDS.with(|c| c.set(c.get() + 1));
}

/// Records a mint: increments `live` and `minted_total`, folding `peak` to the new `live` if it
/// grew. Called exactly once per `RegionHost`, from inside its `OnceCell::get_or_init` closure.
#[cfg(any(test, feature = "test-hooks"))]
fn note_mint() {
    LIVE.with(|live| {
        let count = live.get() + 1;
        live.set(count);
        PEAK.with(|peak| peak.set(peak.get().max(count)));
    });
    MINTED_TOTAL.with(|total| total.set(total.get() + 1));
}

/// The current region-mint metrics for this thread.
#[cfg(any(test, feature = "test-hooks"))]
pub fn region_metrics() -> RegionMetrics {
    RegionMetrics {
        live: LIVE.with(Cell::get),
        peak: PEAK.with(Cell::get),
        minted_total: MINTED_TOTAL.with(Cell::get),
        reach_interned: REACH_INTERNED.with(Cell::get),
        reach_intern_hits: REACH_INTERN_HITS.with(Cell::get),
        reach_retention_folds: REACH_RETENTION_FOLDS.with(Cell::get),
    }
}

/// Zero every counter for this thread. Callers reset before a measured run so `region_metrics()`
/// reads back that run's own contribution only.
#[cfg(any(test, feature = "test-hooks"))]
pub fn reset_region_metrics() {
    LIVE.with(|c| c.set(0));
    PEAK.with(|c| c.set(0));
    MINTED_TOTAL.with(|c| c.set(0));
    REACH_INTERNED.with(|c| c.set(0));
    REACH_INTERN_HITS.with(|c| c.set(0));
    REACH_RETENTION_FOLDS.with(|c| c.set(0));
}

// SAFETY: nothing about drop needs an unsafe obligation here; the impl is gated alongside the
// metrics it feeds, and only decrements `live` when this host actually minted a region — a host that
// never allocated contributed no mint and must not phantom-decrement one.
#[cfg(any(test, feature = "test-hooks"))]
impl<P: StorageProfile> Drop for RegionHost<P> {
    fn drop(&mut self) {
        if self.minted().is_some() {
            LIVE.with(|c| c.set(c.get() - 1));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestProfile;
    impl StorageProfile for TestProfile {
        type FrameOwner = RegionHost<TestProfile>;
    }

    #[test]
    fn lazy_mint_no_region_before_first_access() {
        let host = RegionHost::<TestProfile>::fresh(None);
        assert!(host.minted().is_none());
        let _ = host.region();
        assert!(host.minted().is_some());
    }

    #[test]
    fn only_fresh_eternal_carries_the_eternal_tier() {
        assert!(RegionHost::<TestProfile>::fresh_eternal().is_eternal());
        // A per-call host with no ancestor — a fresh-tail frame — is still per-call.
        assert!(!RegionHost::<TestProfile>::fresh(None).is_eternal());
    }

    #[test]
    fn pins_region_walks_outer_chain() {
        let grandparent = RegionHost::<TestProfile>::fresh(None);
        let parent = RegionHost::<TestProfile>::fresh(Some(Rc::clone(&grandparent)));
        let child = RegionHost::<TestProfile>::fresh(Some(Rc::clone(&parent)));

        // The grandparent mints; parent and child never do, so the walk must pass through them.
        let grandparent_region = grandparent.region();
        assert!(parent.pins_region(grandparent_region));
        assert!(child.pins_region(grandparent_region));

        let other = RegionHost::<TestProfile>::fresh(None);
        assert!(!child.pins_region(other.region()));
    }

    #[test]
    fn metrics_count_mint_and_drop() {
        reset_region_metrics();
        assert_eq!(region_metrics(), RegionMetrics::default());

        {
            let host = RegionHost::<TestProfile>::fresh(None);
            let _ = host.region();
            let metrics = region_metrics();
            assert_eq!(metrics.live, 1);
            assert_eq!(metrics.peak, 1);
            assert_eq!(metrics.minted_total, 1);
        }

        let after_drop = region_metrics();
        assert_eq!(after_drop.live, 0);
        assert_eq!(after_drop.peak, 1);
        assert_eq!(after_drop.minted_total, 1);
    }

    #[test]
    fn drop_without_mint_does_not_decrement_live() {
        reset_region_metrics();
        {
            let _host = RegionHost::<TestProfile>::fresh(None);
        }
        let metrics = region_metrics();
        assert_eq!(metrics.live, 0);
        assert_eq!(metrics.minted_total, 0);
    }
}
