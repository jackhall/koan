//! A list: one run of cells in the region, typed by the union of its elements' filled types
//! ([fill.rs](super::fill)) — or `List<Any>` for a
//! key's candidates, which dispatch alone reads. Its cells are value words, or [`Link`]s when the
//! list is a knot's data node.

use std::cell::Cell;
use std::marker::PhantomData;

use crate::memory::{Bump, BumpAllocator, Writer, resident};
use crate::type_lattice::KType;

use super::fill::hides_more;
use super::hiding::unhidden_cells;
use super::settled::{composite_bits, part_hides, restamp_bits};
use super::{Chain, Knotted, Link, MemoBits, Nothing, Value, Weight, list_type};

/// A list value, resident in the region its cells live in.
#[derive(Debug)]
pub struct List<'cell, X = Nothing, C = Value<'cell, X>> {
    cells: &'cell [C],
    ktype: KType,
    weight: Cell<Weight>,
    /// The knot member a cell's word may hold, which a link cell names only through `C`.
    member: PhantomData<Value<'cell, X>>,
}

/// `items` written as a run of cells into `writer`'s region.
fn filled<'cell, X: Knotted>(
    writer: Writer<'cell, '_>,
    items: impl ExactSizeIterator<Item = Value<'cell, X>>,
) -> &'cell [Value<'cell, X>] {
    let mut items = items;
    writer.fill(items.len(), |_| {
        items
            .next()
            .expect("an exact-size iterator yields its reported length")
    })
}

impl<'cell, X: Knotted> List<'cell, X> {
    /// Lay down `items` as the list's cells. The element type is the union of the cells' filled
    /// types ([`list_type`]) — `Never` for an empty list — and the weight is summed in the same
    /// pass. A cell that would show a field it hides is copied without it ([`unhidden_cells`]).
    pub fn new(
        writer: Writer<'cell, '_>,
        items: impl ExactSizeIterator<Item = Value<'cell, X>>,
        types: &Chain<'_, 'cell, '_, '_>,
        scratch: BumpAllocator<'_>,
    ) -> &'cell List<'cell, X> {
        let cells = filled(writer, items);
        let ktype = list_type(types, scratch, cells.iter().map(Value::concrete_ktype));
        let cells = unhidden_cells(writer, types, scratch, cells, ktype);
        Self::weighed(writer, cells, ktype, types)
    }

    /// Lay down a key's candidates — the functions a `USING` hole or an `EVAL` offer gathers at
    /// one key — typed `List<Any>` without reading any function's type. Dispatch alone reads such
    /// a list, each function by its own type; a quantified registration's type is a scheme, which
    /// no list type joins.
    pub fn of_candidates(
        writer: Writer<'cell, '_>,
        functions: impl ExactSizeIterator<Item = Value<'cell, X>>,
        types: &Chain<'_, 'cell, '_, '_>,
    ) -> &'cell List<'cell, X> {
        Self::typed(writer, functions, KType::LIST_OF_ANY, types)
    }

    /// Lay down `items` under `ktype`, a type the caller knows they satisfy — a view's rebuilt
    /// list, at the type its slot declares — so no type is interned.
    pub fn typed(
        writer: Writer<'cell, '_>,
        items: impl ExactSizeIterator<Item = Value<'cell, X>>,
        ktype: KType,
        types: &Chain<'_, 'cell, '_, '_>,
    ) -> &'cell List<'cell, X> {
        let cells = filled(writer, items);
        let cells = unhidden_cells(writer, types, &Bump::new(), cells, ktype);
        Self::weighed(writer, cells, ktype, types)
    }

    /// A list over value cells already resident in `writer`'s region under `ktype`, weighed as
    /// [`new`](Self::new) weighs them — a retyped data node's arm.
    pub(crate) fn weighed(
        writer: Writer<'cell, '_>,
        cells: &'cell [Value<'cell, X>],
        ktype: KType,
        types: &Chain<'_, 'cell, '_, '_>,
    ) -> &'cell Self {
        let weight = cells.iter().fold(Weight::flat::<Self>(), |weight, cell| {
            weight.plus(cell.weight())
        });
        let bits = composite_bits(types, ktype, cells);
        let hides = cells.iter().any(part_hides);
        Self::from_run(
            writer,
            cells,
            ktype,
            weight.with_bits(bits).with_hides(hides),
        )
    }
}

impl<'cell, X: Knotted> List<'cell, X, Link<'cell, X>> {
    /// Lay down `cells` as a knot's data node under the finished memo `ktype`, which the tie
    /// derived from the cells and the members their edges name.
    pub fn linked(
        writer: Writer<'cell, '_>,
        cells: &[Link<'cell, X>],
        ktype: KType,
    ) -> &'cell Self {
        let weight = cells.iter().fold(Weight::flat::<Self>(), |weight, cell| {
            weight.plus(cell.weight())
        });
        Self::from_run(
            writer,
            writer.fill(cells.len(), |at| cells[at]),
            ktype,
            weight,
        )
    }
}

impl<'cell, X: Copy, C: Copy> List<'cell, X, C> {
    /// A list over cells already resident in `writer`'s region, under a type and weight the caller
    /// already knows — the deep copy's arm.
    pub(crate) fn from_run(
        writer: Writer<'cell, '_>,
        cells: &'cell [C],
        ktype: KType,
        weight: Weight,
    ) -> &'cell Self {
        resident(
            writer,
            List {
                cells,
                ktype,
                weight: Cell::new(weight),
                member: PhantomData,
            },
        )
    }

    /// The same cells under `ktype` — an ascription's retype, sharing the run, its bits
    /// recomputed from the new memo.
    pub fn with_type(
        &self,
        writer: Writer<'cell, '_>,
        ktype: KType,
        types: &Chain<'_, 'cell, '_, '_>,
    ) -> &'cell Self {
        let weight = self.weight.get();
        let weight = weight
            .with_bits(restamp_bits(weight.bits(), types, ktype))
            .with_hides(weight.hides() || hides_more(types, self.ktype, ktype));
        Self::from_run(writer, self.cells, ktype, weight)
    }

    pub fn len(&self) -> usize {
        self.cells.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    pub fn ktype(&self) -> KType {
        self.ktype
    }

    pub fn weight(&self) -> Weight {
        self.weight.get()
    }

    /// Where the value's type memos resolve, as its weight records it.
    pub fn bits(&self) -> MemoBits {
        self.weight.get().bits()
    }

    /// Record that every memo the value reaches resolves in its home half's branch — an exit's
    /// mark on what it adopted. Keeps root-closed.
    pub(crate) fn settle(&self) {
        let weight = self.weight.get();
        self.weight.set(weight.with_bits(MemoBits {
            settled: true,
            ..weight.bits()
        }));
    }

    /// Record that a cell hides a field at its view under the value's own type — the mark a
    /// holder rebuilt over a copy takes ([hiding.rs](super::hiding)) where its parts' bits miss it.
    pub(super) fn hide(&self) {
        self.weight.set(self.weight.get().with_hides(true));
    }
}

/// The runs a read outside `values` reaches only through [the door](super::surface).
impl<'cell, X: Copy> List<'cell, X> {
    pub(super) fn cells(&self) -> &'cell [Value<'cell, X>] {
        self.cells
    }
}

/// A knot's data node's runs, which the knot layer ties and reads.
impl<'cell, X: Copy> List<'cell, X, Link<'cell, X>> {
    pub fn cells(&self) -> &'cell [Link<'cell, X>] {
        self.cells
    }
}
