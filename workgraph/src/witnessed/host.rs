//! [`RegionHost`]: the library-owned region owner with a **lazily minted** region. A workload's
//! per-call frame storage is (or wraps) a `RegionHost<P>` — the region it names is created on first
//! [`region()`](RegionHost::region) access, not at construction, so a frame that never allocates
//! mints nothing. `outer` is the ancestor-frame link [`RegionHost::pins_region`] walks for
//! [`RegionSet`](super::RegionSet) subsumption; the same shape [`RegionOwner`] / [`PinsRegion`] are
//! implemented against everywhere in the workgraph model.

use std::cell::OnceCell;
use std::rc::Rc;

#[cfg(any(test, feature = "test-hooks"))]
use std::cell::Cell;

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
}

impl<P: StorageProfile> RegionHost<P> {
    /// Build a fresh host with no region minted yet, chained to `outer`.
    pub fn fresh(outer: Option<Rc<RegionHost<P>>>) -> Rc<Self> {
        Rc::new(RegionHost {
            region: OnceCell::new(),
            outer,
        })
    }

    /// The backing region, minting it on first call. This is the **sole** mint point: nothing else
    /// in the library or a workload ever constructs a `Region<P>` directly against a `RegionHost`.
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
            Region::new()
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
    pub fn pins_region(&self, region: *const Region<P>) -> bool {
        let mut node = self;
        loop {
            if let Some(minted) = node.minted() {
                if std::ptr::eq(minted, region) {
                    return true;
                }
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
        RegionHost::pins_region(self, region as *const Region<P>)
    }
}

/// Snapshot of the thread-local region-mint counters. `peak` and `minted_total` are monotonic across
/// [`reset_region_metrics`] calls only in the sense that a reset zeroes them; within one measurement
/// window both only grow, while `live` also falls as hosts drop.
#[cfg(any(test, feature = "test-hooks"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RegionMetrics {
    /// Number of `RegionHost`s with a minted region that have not yet dropped.
    pub live: usize,
    /// High-water mark of `live` since the last reset.
    pub peak: usize,
    /// Total number of mints since the last reset (never decremented).
    pub minted_total: usize,
}

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    static LIVE: Cell<usize> = const { Cell::new(0) };
    static PEAK: Cell<usize> = const { Cell::new(0) };
    static MINTED_TOTAL: Cell<usize> = const { Cell::new(0) };
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
    }
}

/// Zero every counter for this thread. Callers reset before a measured run so `region_metrics()`
/// reads back that run's own contribution only.
#[cfg(any(test, feature = "test-hooks"))]
pub fn reset_region_metrics() {
    LIVE.with(|c| c.set(0));
    PEAK.with(|c| c.set(0));
    MINTED_TOTAL.with(|c| c.set(0));
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
        type Families = ();
    }

    #[test]
    fn lazy_mint_no_region_before_first_access() {
        let host = RegionHost::<TestProfile>::fresh(None);
        assert!(host.minted().is_none());
        let _ = host.region();
        assert!(host.minted().is_some());
    }

    #[test]
    fn pins_region_walks_outer_chain() {
        let grandparent = RegionHost::<TestProfile>::fresh(None);
        let parent = RegionHost::<TestProfile>::fresh(Some(Rc::clone(&grandparent)));
        let child = RegionHost::<TestProfile>::fresh(Some(Rc::clone(&parent)));

        // The grandparent mints; parent and child never do, so the walk must pass through them.
        let grandparent_region: *const Region<TestProfile> = grandparent.region();
        assert!(parent.pins_region(grandparent_region));
        assert!(child.pins_region(grandparent_region));

        let other = RegionHost::<TestProfile>::fresh(None);
        let other_region: *const Region<TestProfile> = other.region();
        assert!(!child.pins_region(other_region));
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
