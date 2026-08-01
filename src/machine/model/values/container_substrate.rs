//! [`ContainerSubstrate<'a, C>`] — a region-resident container: its cells in workgraph's
//! [`Sectioned`] storage (semantic order, physically partitioned into runs that each name one
//! interned reach description), a payload-specific index `C` mapping a name / key / position onto a
//! cell index, the interned union over the runs, and the memoized copy cost.
//! [`RecordSubstrate`] (`C = Record<usize>`) is the field substrate behind a record value;
//! [`ListSubstrate`] (`C = ListLayout`) is the element substrate behind a list value;
//! [`DictSubstrate`] (`C = hashbrown::HashMap<KKey, usize>`) is the entry substrate behind a dict
//! value; [`PayloadSubstrate`] (`C = PayloadLayout`) is the single-cell payload substrate behind a
//! `Tagged` or `Wrapped` value.
//!
//! The two borrow memos are **reads on the stored union**, not bits: contains-borrows is "the union
//! is non-empty", borrows-home is the description's own home-relative query. See
//! [design/value-substrates.md § Sectioned reach](../../../../design/value-substrates.md#sectioned-reach).

use hashbrown::HashMap;

use crate::machine::core::{FrameReach, FrameStorage};
use crate::machine::model::types::Record;
use crate::witnessed::{CellRef, Sectioned};

use super::{Held, KKey, KObject};

/// The sectioned cell storage every container substrate holds: [`Held`] cells anchored to the
/// container's own region `'a`, partitioned into runs.
pub type HeldCells<'a> = Sectioned<'a, Held<'static>, FrameStorage>;

/// A cell parted from a container by [`ContainerSubstrate::project`]: the cell reference bundled with
/// a carrier naming exactly its run's stored reach — never a payload and a description as loose
/// parts.
pub type PartedCell<'a> =
    crate::witnessed::Opened<'a, CellRef<Held<'static>>, crate::witnessed::Carrier<FrameStorage>>;

/// The index layout of a [`ListSubstrate`]: a list is positional, so a cell's index *is* its
/// position and there is nothing to store. A distinct unit type rather than `()` so the list
/// substrate family keeps its own arena slot.
pub struct ListLayout;

/// The index layout of a [`PayloadSubstrate`]: exactly one cell, so there is nothing to store. A
/// distinct unit type for the same reason as [`ListLayout`].
pub struct PayloadLayout;

/// A region-resident container: its cells in sectioned storage, the payload-specific index `C` over
/// them, the interned union of their run descriptions, and the memoized copy cost. Immutable after
/// construction — no interior cell writes exist anywhere in the runtime, which is also what keeps a
/// run's description from ever drifting out of exactness with the cells it covers. Born only through
/// the branded door
/// ([`FoldingBrand::alloc_substrate_folded`](crate::machine::core::FoldingBrand::alloc_substrate_folded)),
/// which stores the substrate and hands back a co-located borrow — the cells, their runs and the
/// memos ride together.
pub struct ContainerSubstrate<'a, C> {
    /// The payload-specific index: field name → cell index, dict key → cell index, or a marker where
    /// the layout is implicit (a list's position, a payload's single cell).
    index: C,
    /// The cells and their run partition — workgraph's sectioned storage, resident in this
    /// substrate's own region.
    cells: HeldCells<'a>,
    /// The interned union over the runs — the whole value's reach, the second return of the
    /// sectioned alloc door. Both borrow memos are reads on it.
    reach: &'a FrameReach,
    /// Exact cost in bytes of totally rebuilding this container's reachable structure at a
    /// destination brand. `u64::MAX` (saturated): unpriceable — some transitive cell is a
    /// `KExpression`, which carries no cost memo of its own. A nested `Record`, `List`, `Dict`,
    /// `Tagged`, or `Wrapped` contributes its own cost.
    copy_cost: u64,
}

impl<'a, C> ContainerSubstrate<'a, C> {
    /// Build from the parts the sectioned alloc door produced: the index over the cells, the
    /// sectioned storage and its union description (both from the same
    /// [`Sectioned::build`] call, so they can never be mispaired), and the cost fold.
    pub(crate) fn new(
        index: C,
        cells: HeldCells<'a>,
        reach: &'a FrameReach,
        copy_cost: u64,
    ) -> Self {
        ContainerSubstrate {
            index,
            cells,
            reach,
            copy_cost,
        }
    }

    /// The payload-specific index over the cells.
    pub(crate) fn index(&self) -> &C {
        &self.index
    }

    /// The cells in semantic order — the in-place read every traversal (equality, rendering, a
    /// copying rebuild) runs under the pin that reaches this substrate at all. A cell that
    /// *travels* takes [`Self::project`] instead.
    pub fn cells(&self) -> &'a [&'a Held<'a>] {
        self.cells.cells()
    }

    /// The cell at `index`, or `None` past the end.
    pub fn cell(&self, index: usize) -> Option<&'a Held<'a>> {
        self.cells.cell(index)
    }

    /// Number of cells.
    pub fn len(&self) -> usize {
        self.cells.len()
    }

    /// Whether the container holds no cells.
    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    /// **The parting seam.** Hand the cell at `index` out bundled with a carrier naming exactly its
    /// run's stored reach — read off the run, never derived by a subset walk over the container. The
    /// bundle is `'a`-confined, so a cell that genuinely relocates has to pass
    /// [`Opened::reseal`](crate::witnessed::Opened::reseal), which is where the mint happens.
    pub fn project(&self, index: usize) -> Option<PartedCell<'a>> {
        self.cells.project(index)
    }

    /// This container's whole stored reach — the interned union over its runs. The mint source a
    /// pinned relocation of this container reads, and what both borrow memos answer from.
    pub fn reach(&self) -> &'a FrameReach {
        self.reach
    }

    /// Whether any cell reaches a region at all — the union being non-empty. Conservative relative
    /// to [`Self::borrows_home`]: it asks about *any* region, not the home-relative question the
    /// cost decision needs.
    pub fn contains_borrows(&self) -> bool {
        !self.reach.is_empty()
    }

    /// Exact cost in bytes of totally rebuilding this container at a destination brand, or
    /// `u64::MAX` when unpriceable — see the field's own doc.
    pub fn copy_cost(&self) -> u64 {
        self.copy_cost
    }

    /// Whether some borrow leaf points into this container's own home region — the union's own
    /// home-relative query. Exact: a cell resident in this region contributes no home member (the
    /// run-level self rule at the alloc door), so a set bit means a genuine borrow leaf, which is
    /// precisely the gate the cost-driven copy decision reads.
    ///
    /// Home is the **substrate's** home — the region its storage and description live in, recorded
    /// as the description's host. A pin-bind pointer-copies a wrapper value into another region
    /// while it keeps sharing this substrate, so a wrapper's residence can differ; a consumer
    /// asking a residence-relative question checks the two match first (as the cost decision's
    /// home-crossing test does) or passes its region explicitly ([`Self::reach`] +
    /// `pins_region`).
    pub fn borrows_home(&self) -> bool {
        self.reach.borrows_home()
    }
}

/// The field substrate a record value borrows — [`ContainerSubstrate`] indexed by field name. Cells
/// are in declaration order; equality is order-blind (see [`Record`]).
pub(crate) type RecordSubstrate<'a> = ContainerSubstrate<'a, Record<usize>>;

impl<'a> RecordSubstrate<'a> {
    /// The cell named `field`, or `None` when the record has no such field.
    pub fn field(&self, field: &str) -> Option<&'a Held<'a>> {
        self.index().get(field).and_then(|at| self.cell(*at))
    }

    /// The cell index of `field`, or `None` when the record has no such field — what a projection
    /// resolves a field name to before parting the cell ([`Self::project`]).
    pub fn field_index(&self, field: &str) -> Option<usize> {
        self.index().get(field).copied()
    }

    /// The fields in declaration order, as `(name, cell)` pairs.
    pub fn fields(&self) -> impl Iterator<Item = (&str, &'a Held<'a>)> {
        let cells = self.cells();
        self.index()
            .iter()
            .map(move |(name, at)| (name.as_str(), cells[*at]))
    }

    /// The field names in declaration order.
    pub fn field_names(&self) -> impl Iterator<Item = &str> {
        self.index().keys().map(String::as_str)
    }
}

/// The element substrate a list value borrows — [`ContainerSubstrate`] with the implicit positional
/// layout, so a cell's index is its position.
pub(crate) type ListSubstrate<'a> = ContainerSubstrate<'a, ListLayout>;

impl<'a> ListSubstrate<'a> {
    /// The elements in index order.
    pub fn elements(&self) -> &'a [&'a Held<'a>] {
        self.cells()
    }
}

/// The entry substrate a dict value borrows — [`ContainerSubstrate`] indexed by the concrete scalar
/// [`KKey`]. The index is frozen at construction (last-wins dedup happens in the transient
/// construction map) and never written again; cell order follows the construction map's iteration
/// order, so entry order is unspecified. The index block is a default-`Global` heap allocation the
/// substrate owns and drops at region death — a `hashbrown` table so a future region-`Allocator` swap
/// is a zero-payload-churn change. Every key is `Copy` and its string bytes are region-hosted, so
/// the table holds no per-key allocation of its own.
pub(crate) type DictSubstrate<'a> = ContainerSubstrate<'a, HashMap<KKey<'a>, usize>>;

impl<'a> DictSubstrate<'a> {
    /// The cell stored under `key`, or `None` when the dict has no such entry. `key` may borrow
    /// anywhere — lookup is by content, so a probe built at the call site matches a stored key
    /// whose bytes live in this dict's region.
    pub fn entry(&self, key: &KKey<'_>) -> Option<&'a Held<'a>> {
        self.index().get(key).and_then(|at| self.cell(*at))
    }

    /// The entries as `(key, cell)` pairs — arbitrary order; look one up with [`Self::entry`].
    pub fn entries(&self) -> impl Iterator<Item = (&KKey<'a>, &'a Held<'a>)> {
        let cells = self.cells();
        self.index().iter().map(move |(key, at)| (key, cells[*at]))
    }

    /// The keys, in the same arbitrary order [`Self::entries`] yields.
    pub fn keys(&self) -> impl Iterator<Item = &KKey<'a>> {
        self.index().keys()
    }
}

/// The single-cell payload substrate an identity-carrying composite borrows — a `Tagged` value's
/// `value` and a `Wrapped` value's `inner` both ride one of these: exactly one cell (a tagged/wrapped
/// value is always an object, never a first-class type) plus its run and the memos. One substrate
/// family shared by both carriers, born only through the fold door.
pub(crate) type PayloadSubstrate<'a> = ContainerSubstrate<'a, PayloadLayout>;

impl<'a> PayloadSubstrate<'a> {
    /// The single payload cell's object. Infallible by the door's own invariant: `alloc_payload` is
    /// the sole construction site and it hands in exactly one `Held::Object`.
    pub fn payload(&self) -> &'a KObject<'a> {
        self.cell(0)
            .expect("a payload substrate is built with exactly one cell")
            .object()
    }
}

/// One [`Held`] cell's flat size in bytes — the [`Held`] discriminant plus its owned payload,
/// counted for a cost memo. `Held` is invariant in its lifetime, so its size is lifetime-independent.
fn held_flat_size() -> u64 {
    std::mem::size_of::<Held<'static>>() as u64
}

/// The per-cell copy-cost rule shared by every substrate door: a type-channel cell costs one flat
/// [`Held`]; an object cell defers to [`object_copy_cost`].
pub(crate) fn held_copy_cost(h: &Held<'_>) -> u64 {
    match h {
        Held::Type(_) | Held::UnresolvedType(_) => held_flat_size(),
        Held::Object(o) => object_copy_cost(o),
    }
}

/// The object-level copy-cost rule (the [`Held::Object`] arm of [`held_copy_cost`]): the bytes of
/// totally rebuilding this object at a destination brand. A scalar costs one flat [`Held`]; a
/// `KString` adds its byte length; a `KFunction` / `Module` is a borrow leaf that rides the transfer
/// and rebuilds nothing (**0**); a nested `Record`, `List`, `Dict`, `Tagged`, or `Wrapped`
/// contributes its own memoized cost (a `Tagged`'s tag bytes stay out — short, the same
/// negligible approximation a `KString` cell already takes for its own discriminant); a
/// `KExpression` is unpriceable (`u64::MAX`), carrying no cost memo of its own.
fn object_copy_cost(o: &KObject<'_>) -> u64 {
    match o {
        KObject::Number(_) | KObject::Bool(_) | KObject::Null => held_flat_size(),
        KObject::KString(s) => held_flat_size().saturating_add(s.len() as u64),
        KObject::KFunction(_) | KObject::Module(_) => 0,
        KObject::Record(substrate, _) => substrate.copy_cost(),
        KObject::List(substrate, _) => substrate.copy_cost(),
        KObject::Dict(substrate, _) => substrate.copy_cost(),
        KObject::Tagged { value, .. } => value.copy_cost(),
        KObject::Wrapped { inner, .. } => inner.copy_cost(),
        KObject::KExpression(_) => u64::MAX,
    }
}
