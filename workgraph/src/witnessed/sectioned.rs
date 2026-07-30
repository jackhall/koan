//! Sectioned container storage: reach evidence stored at **sub-value** granularity. A
//! [`Sectioned`] holds its payload cells in semantic order, physically partitioned into contiguous
//! **runs** that each pair a span of cells with one interned `&ReachDescription`. A cell's reach is
//! therefore a stored, O(log runs)-readable fact, so a seam that parts a cell from its container
//! reads reach instead of re-deriving it by walking the value
//! ([design/sectioned-reach.md](../../design/sectioned-reach.md) § Sectioned storage).
//!
//! The cell type is the embedder's, named as a [`Reattachable`] family `K`; no embedder type enters
//! this module. Everything a container is made of lives in the destination region and is anchored to
//! its `'a`: the cells arrive as `&'a K::At<'a>` (content == borrow == `'a`, so a cell is already
//! resident where the caller allocated it), each run's description is interned in that region's reach
//! side table, and the two slices holding the index→cell mapping and the run partition are bumped
//! into that region ([`Region::bump_slice`]). So one pin covers a projected cell and its reach
//! together, and a cycle among them is harmless — the region dies all at once.
//!
//! That is what makes a [`Sectioned`] `Copy` and **`Drop`-free**: a frame teardown never walks a
//! container. Cell *layout* stays the embedder's — workgraph holds the mapping and the partition,
//! never the bytes.
//!
//! [`Sectioned::project`] therefore parts a cell as a **bundled** carrier —
//! `Opened<'a, CellRef<K>, Carrier<F>>` — never a payload and a description as loose parts. The
//! pairing of a cell with its own reach becomes a type invariant, the cell arrives as a *reference*
//! (so parting costs no clone however expensive the cell is), and the `'a` the [`Opened`] state
//! carries keeps the projection inside the region. [`Opened`] rather than
//! [`Sealed`](super::Sealed) for exactly that last reason: a seal is lifetime-free by construction
//! — the dormant form that may outlive anything and names its coverage at the open — whereas a
//! projection must not outlive its container's region at all. A cell that genuinely travels takes
//! [`Opened::reseal`], which is the mint-consuming relocation seam.
//!
//! Containers are built through one door, [`Sectioned::build`], which takes a per-cell
//! `(payload, reach verdict)` and owns everything downstream of it — grouping into runs, interning,
//! pin folding, and the value-level union. Interning is what makes grouping cheap: within one
//! region a description's *address* is its member set ([`Region::intern_reach`]), so a run boundary
//! is one pointer compare per cell rather than a set comparison. No `unsafe`.

use std::marker::PhantomData;
use std::ops::Range;

use super::{
    Carrier, Opened, PinBundle, PinsRegion, ReachDescription, Reattachable, Region, RegionHandle,
    RegionOwner, StepCoverage, StorageProfile,
};

/// One physical partition: the index its span starts at, paired with the interned description that
/// is **exactly** its cells' shared reach. The span's end is the next run's start (or the
/// container's length for the last run), so a run costs one `usize` and one thin reference.
///
/// `Copy`, hence `Drop`-free — the bound [`Region::bump_slice`] requires, and what keeps a container
/// free at region teardown.
struct Run<'a, F: PinsRegion + 'static> {
    start: usize,
    reach: &'a ReachDescription<F>,
}

// Manual impls: a derive would bound `F: Copy`, which neither field needs.
impl<F: PinsRegion + 'static> Clone for Run<'_, F> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<F: PinsRegion + 'static> Copy for Run<'_, F> {}

/// [`Reattachable`] family for a **reference to** a cell of family `K` — the erased form
/// [`Sectioned::project`] seals. `At<'r>` is `&'r K::At<'r>`: content == borrow == `'r`, the tight
/// no-free-lifetime shape [`Region::alloc_resident`] hands back, so a re-anchored projection cannot
/// be widened past its pin.
///
/// Being a reference is what makes the projection cheap and universally openable: it is `Copy`
/// whatever `K` is, so [`Sealed::open_with`]'s `At<'static>: Copy` bound is satisfied for free even
/// where the cell itself is an expensive owned aggregate.
pub struct CellRef<K>(PhantomData<K>);

// SAFETY: `&'r K::At<'r>` is a pointer, whose layout is identical for every choice of `'r`.
unsafe impl<K: Reattachable + 'static> Reattachable for CellRef<K> {
    type At<'r> = &'r K::At<'r>;
}

/// A container whose cells carry reach at sub-value granularity: cells of the embedder's family `K`
/// in semantic order, region-resident in `'a`, physically partitioned into contiguous runs that each
/// name one interned [`ReachDescription`] hosted in that same region.
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
pub struct Sectioned<'a, K: Reattachable + 'static, F: PinsRegion + 'static> {
    /// The index→cell mapping, in semantic order, bumped into the region. Each cell is
    /// `&'a K::At<'a>` — the tight no-free-lifetime shape, so a projection out of it is
    /// `'a`-confined by its own type.
    cells: &'a [&'a K::At<'a>],
    /// Ascending by `start`, contiguous and covering; empty exactly when `cells` is. Bumped into the
    /// region alongside `cells`, so the partition is region state rather than a heap buffer a frame
    /// drop would have to free.
    runs: &'a [Run<'a, F>],
}

// Manual impls: a derive would bound `K: Copy` / `F: Copy`, which two shared slices do not need.
// `Copy` is the point of the type — a container is region state a holder names, not one it owns.
impl<K: Reattachable + 'static, F: PinsRegion + 'static> Clone for Sectioned<'_, K, F> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<K: Reattachable + 'static, F: PinsRegion + 'static> Copy for Sectioned<'_, K, F> {}

/// The reach verdict a caller supplies per input cell — the embedder's whole reach obligation at
/// the door, alongside the payload itself.
pub enum CellReach<'r, F: PinsRegion> {
    /// Fully owned at the destination — a copied cell, or owned data. Lands in an empty-reach run
    /// with no walk.
    Owned,
    /// The cell keeps borrowing its source: its own stored description (which is exact, where the
    /// envelope coverage a step holds is generally wider), plus — under the run-level self rule
    /// ([`Sectioned::build`]) — its home region whenever that is not the destination itself.
    /// `coverage` is the caller's holder-rule proof — it pins every region the description names for
    /// the whole call, which is what makes reading the description's members back out of it sound.
    Pinned {
        reach: &'r ReachDescription<F>,
        coverage: StepCoverage<F>,
    },
    /// A born-borrowing seed: reach declared at construction from pins the caller already holds,
    /// rather than composed from a stored description.
    Seed(StepCoverage<F>),
}

/// One input to [`Sectioned::build`]: a region-resident cell and its reach verdict. Pairing them in
/// one value is what makes a verdict-per-cell mismatch unrepresentable — there is no separate
/// cells-and-verdicts pair of sequences to fall out of step.
pub struct CellInput<'a, 'r, K: Reattachable, F: PinsRegion> {
    /// The cell, already resident in storage that outlives the destination region handle.
    pub payload: &'a K::At<'a>,
    pub reach: CellReach<'r, F>,
}

impl<'a, K: Reattachable + 'static, F: PinsRegion + 'static> Sectioned<'a, K, F> {
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

    /// The reach of the cell at `index`: the description of the run covering it, found by binary
    /// search over the run starts.
    ///
    /// A **container-level query**, not a hand-out: it answers a question about reach under the pin
    /// the caller already holds (it holds the container) and yields no cell. The seam that parts a
    /// cell *from* the container is [`Self::project`], which bundles.
    pub fn reach_at(&self, index: usize) -> Option<&'a ReachDescription<F>> {
        if index >= self.cells.len() {
            return None;
        }
        // Runs are ascending and cover every index, so the covering run is the last one starting at
        // or before `index` — and index 0 is always covered, so the partition point is never 0.
        let covering = self.runs.partition_point(|run| run.start <= index) - 1;
        Some(self.runs[covering].reach)
    }

    /// The cells in semantic order — the **in-place read** twin of [`Self::reach_at`]: where that
    /// answers a reach question yielding no cell, this hands out cells yielding no reach. Neither is
    /// a parting seam, and neither needs to be: both run under the pin the caller already holds (it
    /// holds the container), and a cell reference is `&'a K::At<'a>`, so it is confined to the
    /// container's own region by its type. This is what an embedder's in-place traversals —
    /// equality, rendering, a copying rebuild — read, none of which relocates a cell.
    ///
    /// The seam that *parts* a cell from the container is [`Self::project`], which bundles it with
    /// its reach; the pairing is a type invariant exactly where a cell can travel.
    pub fn cells(&self) -> &'a [&'a K::At<'a>] {
        self.cells
    }

    /// The cell at `index`, or `None` past the end — [`Self::cells`] at one index.
    pub fn cell(&self, index: usize) -> Option<&'a K::At<'a>> {
        self.cells.get(index).copied()
    }

    /// The runs, as `(span, reach)` pairs in ascending order — the container-level query an
    /// embedder's contains-borrows / borrows-home memos fold over instead of walking cells.
    pub fn runs(&self) -> impl Iterator<Item = (Range<usize>, &'a ReachDescription<F>)> + 'a {
        let (cells, runs) = (self.cells, self.runs);
        runs.iter().enumerate().map(move |(position, run)| {
            let end = runs
                .get(position + 1)
                .map_or(cells.len(), |next| next.start);
            (run.start..end, run.reach)
        })
    }

    /// **The parting seam.** Hand the cell at `index` out as a bundled carrier: a reference to the
    /// cell, sealed under a [`Carrier`] naming exactly that cell's run reach.
    ///
    /// Bundled rather than a `(payload, description)` pair, for the reason the whole module is: the
    /// value↔reach pairing becomes a type invariant instead of caller discipline, and the cell
    /// travels as a reference, so parting is free however expensive the cell is.
    ///
    /// [`Opened`] rather than [`Sealed`](super::Sealed) because confinement has to be a compile
    /// error: a seal is lifetime-free by construction (that is what makes it the dormant storage
    /// form) and would outlive the region freely, naming its coverage only at the open. An
    /// `Opened<'a, …>` carries `'a`, so both halves are confined by ordinary borrow checking — the
    /// cell reference is `&'a K::At<'a>` and the carrier's description is hosted in that same
    /// region. A cell that genuinely travels passes [`Opened::reseal`], the mint-consuming
    /// relocation seam. The `'a` here is the destination region's own lifetime rather than a borrow
    /// of a pin, which is sound for [`Opened::adopted`]'s stated reason: [`Self::build`] retained
    /// every cell's pins into the region before this projection could exist.
    ///
    /// ```
    /// use workgraph::witnessed::doctest_fixture::{fresh_cart, RefFamily};
    /// use workgraph::witnessed::{CellInput, CellReach, RegionHandle, Sectioned};
    ///
    /// let cart = fresh_cart();
    /// let handle = RegionHandle::from_owner(&*cart);
    /// // `alloc_resident` hands back `&'a RefFamily::At<'a>` — a cell reference, resident.
    /// let cell: &&u32 = handle.alloc_resident::<RefFamily>(&7);
    /// let (container, _) = Sectioned::<RefFamily, _>::build(
    ///     handle,
    ///     vec![CellInput { payload: cell, reach: CellReach::Owned }],
    /// );
    /// // The projection is one value: the cell reference and its reach, paired.
    /// let parted = container.project(0).expect("index is covered");
    /// assert_eq!(**parted.value(), 7);
    /// // Its reach rides along, so a consumer never has to be handed it separately.
    /// assert!(!parted.borrows_home());
    /// ```
    ///
    /// ```compile_fail
    /// // A parted cell cannot outlive its container's region: the `Opened` state carries `'a`.
    /// use workgraph::witnessed::doctest_fixture::{fresh_cart, RefFamily};
    /// use workgraph::witnessed::{CellInput, CellReach, RegionHandle, Sectioned};
    ///
    /// let cart = fresh_cart();
    /// let handle = RegionHandle::from_owner(&*cart);
    /// // `alloc_resident` hands back `&'a RefFamily::At<'a>` — a cell reference, resident.
    /// let cell: &&u32 = handle.alloc_resident::<RefFamily>(&7);
    /// let (container, _) = Sectioned::<RefFamily, _>::build(
    ///     handle,
    ///     vec![CellInput { payload: cell, reach: CellReach::Owned }],
    /// );
    /// let parted = container.project(0).unwrap();
    /// drop(cart);
    /// let _ = parted;
    /// ```
    ///
    /// ```compile_fail
    /// // The run's description reference is confined the same way — outliving the container is not
    /// // enough, the region has to outlive it too.
    /// use workgraph::witnessed::doctest_fixture::{fresh_cart, RefFamily};
    /// use workgraph::witnessed::{CellInput, CellReach, RegionHandle, Sectioned};
    ///
    /// let cart = fresh_cart();
    /// let handle = RegionHandle::from_owner(&*cart);
    /// // `alloc_resident` hands back `&'a RefFamily::At<'a>` — a cell reference, resident.
    /// let cell: &&u32 = handle.alloc_resident::<RefFamily>(&7);
    /// let (container, _) = Sectioned::<RefFamily, _>::build(
    ///     handle,
    ///     vec![CellInput { payload: cell, reach: CellReach::Owned }],
    /// );
    /// let reach = container.reach_at(0).unwrap();
    /// drop(container);
    /// drop(cart);
    /// assert!(reach.is_empty());
    /// ```
    pub fn project(&self, index: usize) -> Option<Opened<'a, CellRef<K>, Carrier<F>>> {
        // The stored cell reference is already `&'a`, so copying it out carries the region lifetime
        // rather than this `&self` borrow.
        let cell: &'a K::At<'a> = *self.cells.get(index)?;
        let reach = self.reach_at(index)?;
        Some(Opened::adopted(cell, Carrier::new(reach)))
    }

    /// The alloc door: build a sectioned container resident in `dest` from per-cell
    /// `(payload, reach verdict)` inputs, returning it alongside its **value-level** description —
    /// what a whole-value carrier stores, so carriers keep their single-`&ReachDescription` shape
    /// unchanged.
    ///
    /// Each input cell arrives already resident as `&'a K::At<'a>`, tying it to the same `'a` the
    /// destination handle carries — so whatever pin keeps `dest`'s region alive covers both a
    /// projected cell and its run description. Cell *storage* is the embedder's: it allocates its
    /// cell block through its own [`Stored`](super::Stored) family and hands the resident borrows
    /// in, rather than workgraph re-declaring a cell family it has no other use for.
    ///
    /// Per input, the mint source is the verdict read literally: nothing for
    /// [`CellReach::Owned`] (so owned data costs no walk), the stored description's members — plus
    /// its home region under the run-level self rule below — for [`CellReach::Pinned`], the declared
    /// pins for [`CellReach::Seed`]. Each
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
        inputs: Vec<CellInput<'a, '_, K, F>>,
    ) -> (Self, &'a ReachDescription<F>)
    where
        W: StorageProfile<FrameOwner = F>,
        F: RegionOwner<Region = Region<W>>,
    {
        let mut cells: Vec<&'a K::At<'a>> = Vec::with_capacity(inputs.len());
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
                    // The cell's own residence joins its members only when it is somewhere *else* —
                    // the run-level self rule. A cell already resident in `dest` is covered by
                    // `dest`'s own liveness, so naming it would make every container holding a
                    // co-resident sub-container read as borrowing its own home, and the
                    // borrows-home memo folded from these runs would stop answering the question it
                    // exists to answer: does a *borrow leaf* point home. A cell resident elsewhere
                    // is a genuine cross-region borrow and its host is folded in as an ordinary
                    // member — nothing else would pin it.
                    if !reach.with_home_region(|home| std::ptr::eq(home, dest.region())) {
                        source.insert(reach.host_owner());
                    }
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

        // Bump both slices into the region: the container becomes `Copy` region state, so a frame
        // teardown releases it with the chunk instead of walking it.
        let sectioned = Sectioned {
            cells: dest.region().bump_slice(&cells),
            runs: dest.region().bump_slice(&runs),
        };
        (sectioned, value_level)
    }
}
