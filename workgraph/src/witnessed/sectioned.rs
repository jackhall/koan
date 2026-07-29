//! Sectioned container storage: reach evidence stored at **sub-value** granularity. A
//! [`Sectioned`] holds its payload cells in semantic order, physically partitioned into contiguous
//! **runs** that each pair a span of cells with one interned `&ReachDescription`. A cell's reach is
//! therefore a stored, O(log runs)-readable fact, so a seam that parts a cell from its container
//! reads reach instead of re-deriving it by walking the value
//! (design/sectioned-reach.md § Sectioned storage).
//!
//! The payload type `P` is the embedder's; no embedder type enters this module. Confinement is
//! ordinary borrow checking: `'a` is the destination region's lifetime, carried by every run's
//! description reference and (in practice) by the payload itself, so a projected cell cannot
//! outlive its container's region without passing a mint-consuming relocation seam. No `unsafe`.
//!
//! Containers are built through one door, [`Sectioned::build`], which takes a per-cell
//! `(payload, reach verdict)` and owns everything downstream of it — grouping into runs, interning,
//! pin folding, and the value-level union. Interning is what makes grouping cheap: within one
//! region a description's *address* is its member set ([`Region::intern_reach`]), so a run boundary
//! is one pointer compare per cell rather than a set comparison.

use std::ops::Range;

use super::{
    PinBundle, PinsRegion, ReachDescription, Region, RegionHandle, RegionOwner, StepCoverage,
    StorageProfile,
};

/// One physical partition: the index its span starts at, paired with the interned description that
/// is **exactly** its cells' shared reach. The span's end is the next run's start (or the
/// container's length for the last run), so a run costs one `usize` and one thin reference.
struct Run<'a, F: PinsRegion> {
    start: usize,
    reach: &'a ReachDescription<F>,
}

/// A payload-generic container whose cells carry reach at sub-value granularity: cells in semantic
/// order, physically partitioned into contiguous runs that each name one interned
/// [`ReachDescription`] hosted in the container's own region.
///
/// **Immutable after the door.** There is no push / insert / remove, so a run's description can
/// never drift out of exactness with the cells it covers. Build one with [`Self::build`].
///
/// A run's description is precisely the shared reach of its cells — adjacency decides sharing, so
/// the same reach appearing in non-adjacent runs makes two run entries naming one interned
/// description. That exactness is what makes projection release-exact: a cell parted from the
/// container carries exactly its own reach, never the container's union. A single-run container
/// (all-owned, or one shared reach) is the fast path: one description, no per-cell cost.
/// Alternating owned and borrowing cells degrade to runs of length one — the per-cell-envelope cost
/// floor, never worse than storing reach on every cell.
pub struct Sectioned<'a, P, F: PinsRegion> {
    /// The payloads, in semantic order.
    cells: Vec<P>,
    /// Ascending by `start`, contiguous and covering; empty exactly when `cells` is.
    runs: Vec<Run<'a, F>>,
}

/// The reach verdict a caller supplies per input cell — the embedder's whole reach obligation at
/// the door, alongside the payload itself.
pub enum CellReach<'r, F: PinsRegion> {
    /// Fully owned at the destination — a copied input, or owned data. Lands in an empty-reach run
    /// with no walk.
    Owned,
    /// The input keeps borrowing its source: its own stored description (which is exact, where the
    /// envelope coverage a step holds is generally wider) plus its home region, folded in as an
    /// ordinary member. `coverage` is the caller's holder-rule proof — it pins every region the
    /// description names for the whole call, which is what makes reading the description's members
    /// back out of it sound.
    Pinned {
        reach: &'r ReachDescription<F>,
        coverage: StepCoverage<F>,
    },
    /// A born-borrowing seed: reach declared at construction from pins the caller already holds,
    /// rather than composed from an input's stored description.
    Seed(StepCoverage<F>),
}

/// One input to [`Sectioned::build`]: the payload and its reach verdict.
pub struct CellInput<'r, P, F: PinsRegion> {
    pub payload: P,
    pub reach: CellReach<'r, F>,
}

impl<'a, P, F: PinsRegion> Sectioned<'a, P, F> {
    /// Number of cells.
    pub fn len(&self) -> usize {
        self.cells.len()
    }

    /// Whether the container holds no cells.
    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    /// Number of physical runs — the container's reach-storage cost.
    pub fn run_count(&self) -> usize {
        self.runs.len()
    }

    /// Whether the whole container shares one reach — the fast path, where reach costs one
    /// description and no per-cell lookup.
    pub fn is_single_run(&self) -> bool {
        self.runs.len() == 1
    }

    /// The payload at `index`.
    pub fn cell(&self, index: usize) -> Option<&P> {
        self.cells.get(index)
    }

    /// The reach of the cell at `index`: the description of the run covering it, found by binary
    /// search over the run starts.
    pub fn reach_at(&self, index: usize) -> Option<&'a ReachDescription<F>> {
        if index >= self.cells.len() {
            return None;
        }
        // Runs are ascending and cover every index, so the covering run is the last one starting at
        // or before `index` — and index 0 is always covered, so the partition point is never 0.
        let covering = self.runs.partition_point(|run| run.start <= index) - 1;
        Some(self.runs[covering].reach)
    }

    /// The parting seam's read: a cell's payload alongside exactly its own reach. Both are
    /// `'a`-confined to the container's region — the payload through its own type, the description
    /// through the run's reference — so neither can outlive the region without passing a seam that
    /// relocates the reach into a destination. The compiler enforces the seam.
    ///
    /// A projection read inside the region's life is ordinary:
    ///
    /// ```
    /// use workgraph::witnessed::doctest_fixture::{fresh_cart, RefFamily};
    /// use workgraph::witnessed::{CellInput, CellReach, RegionHandle, Sectioned};
    /// let cart = fresh_cart();
    /// let handle = RegionHandle::from_owner(&*cart);
    /// let value: &u32 = handle.alloc_resident::<RefFamily>(&7);
    /// let (container, _) = Sectioned::build(
    ///     handle,
    ///     vec![CellInput { payload: value, reach: CellReach::Owned }],
    /// );
    /// let (payload, reach) = container.project(0).unwrap();
    /// assert_eq!(**payload, 7);
    /// assert!(reach.is_empty());
    /// ```
    ///
    /// ```compile_fail
    /// // The projected PAYLOAD is confined: it cannot outlive the container's region.
    /// use workgraph::witnessed::doctest_fixture::{fresh_cart, RefFamily};
    /// use workgraph::witnessed::{CellInput, CellReach, RegionHandle, Sectioned};
    /// let cart = fresh_cart();
    /// let handle = RegionHandle::from_owner(&*cart);
    /// let value: &u32 = handle.alloc_resident::<RefFamily>(&7);
    /// let (container, _) = Sectioned::build(
    ///     handle,
    ///     vec![CellInput { payload: value, reach: CellReach::Owned }],
    /// );
    /// let (payload, _) = container.project(0).unwrap();
    /// drop(cart);
    /// assert_eq!(**payload, 7);
    /// ```
    ///
    /// ```compile_fail
    /// // The run's DESCRIPTION reference is confined the same way — outliving the container is not
    /// // enough, the region has to outlive it too.
    /// use workgraph::witnessed::doctest_fixture::{fresh_cart, RefFamily};
    /// use workgraph::witnessed::{CellInput, CellReach, RegionHandle, Sectioned};
    /// let cart = fresh_cart();
    /// let handle = RegionHandle::from_owner(&*cart);
    /// let value: &u32 = handle.alloc_resident::<RefFamily>(&7);
    /// let (container, _) = Sectioned::build(
    ///     handle,
    ///     vec![CellInput { payload: value, reach: CellReach::Owned }],
    /// );
    /// let reach = container.project(0).unwrap().1;
    /// drop(container);
    /// drop(cart);
    /// assert!(reach.is_empty());
    /// ```
    pub fn project(&self, index: usize) -> Option<(&P, &'a ReachDescription<F>)> {
        Some((self.cell(index)?, self.reach_at(index)?))
    }

    /// The runs, as `(span, reach)` pairs in ascending order — what an embedder's
    /// contains-borrows / borrows-home memos fold over instead of walking cells.
    pub fn runs(&self) -> impl Iterator<Item = (Range<usize>, &'a ReachDescription<F>)> + '_ {
        let total = self.cells.len();
        self.runs.iter().enumerate().map(move |(position, run)| {
            let end = self.runs.get(position + 1).map_or(total, |next| next.start);
            (run.start..end, run.reach)
        })
    }
}

impl<'a, P, F: PinsRegion> Sectioned<'a, P, F> {
    /// The alloc door: build a sectioned container resident in `dest` from per-cell
    /// `(payload, reach verdict)` inputs, returning it alongside its **value-level** description —
    /// what a whole-value carrier stores, so carriers keep their single-`&ReachDescription` shape
    /// unchanged.
    ///
    /// Per input, the mint source is the verdict read literally: nothing for
    /// [`CellReach::Owned`] (so owned data costs no walk), the stored description's members plus
    /// its home region for [`CellReach::Pinned`], the declared pins for [`CellReach::Seed`]. Each
    /// source is minted into `dest` and **retained** there ([`Region::retain_for`]): a sectioned
    /// container is region-resident, so its liveness home is the region's union bundle, and the
    /// fold is skipped whenever the mint is an intern hit.
    ///
    /// Runs come out of the mints for free — a boundary is where the minted description's address
    /// changes, and within one region that address *is* the member set. The value-level description
    /// is the mint over the union of the per-cell sources.
    ///
    /// The union accumulates the **pre-mint** source bundles, not the bundles the mints hand back:
    /// the self rule strips `dest`'s own region from a returned bundle while leaving it in the
    /// description, so folding the returned bundles would drop home from the value-level
    /// description exactly when a cell genuinely borrows into `dest`.
    pub fn build<W>(
        dest: RegionHandle<'a, W>,
        inputs: Vec<CellInput<'_, P, F>>,
    ) -> (Self, &'a ReachDescription<F>)
    where
        W: StorageProfile<FrameOwner = F>,
        F: RegionOwner<Region = Region<W>>,
    {
        let mut cells: Vec<P> = Vec::with_capacity(inputs.len());
        let mut runs: Vec<Run<'a, F>> = Vec::new();
        let mut union = PinBundle::empty();

        for (index, CellInput { payload, reach }) in inputs.into_iter().enumerate() {
            let source = match reach {
                CellReach::Owned => PinBundle::empty(),
                CellReach::Pinned { reach, coverage } => {
                    // `coverage` pins every region `reach` names for the whole arm, which is what
                    // makes the upgrade below succeed under the holder rule; it drops at the end.
                    let _held = coverage;
                    let mut source = reach.to_bundle();
                    source.insert(reach.host_owner());
                    source
                }
                CellReach::Seed(coverage) => coverage.0,
            };
            let (description, bundle) = ReachDescription::mint(dest, &[&source]);
            dest.region().retain_for(description, bundle);
            union.absorb(source);

            // A new run starts wherever the interned description changes. Pointer identity is
            // member-set equality within a region, so this is one compare per cell.
            if runs
                .last()
                .is_none_or(|run| !std::ptr::eq(run.reach, description))
            {
                runs.push(Run {
                    start: index,
                    reach: description,
                });
            }
            cells.push(payload);
        }

        let (value_level, bundle) = ReachDescription::mint(dest, &[&union]);
        dest.region().retain_for(value_level, bundle);
        (Sectioned { cells, runs }, value_level)
    }
}
