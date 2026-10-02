//! A list: one run of cells in the region, typed by the join of its elements — or `List<Any>` for a
//! key's candidates, which dispatch alone reads. Its cells are value words, or [`Link`]s when the
//! list is a knot's data node.

use std::marker::PhantomData;

use crate::memory::{BumpAllocator, Writer, resident};
use crate::type_lattice::{KType, TypeRegistry};

use super::{Knotted, Link, Nothing, Value, Weight, list_type};

/// A list value, resident in the region its cells live in.
#[derive(Clone, Copy, Debug)]
pub struct List<'cell, X = Nothing, C = Value<'cell, X>> {
    cells: &'cell [C],
    ktype: KType,
    weight: Weight,
    /// The knot member a cell's word may hold, which a link cell names only through `C`.
    member: PhantomData<Value<'cell, X>>,
}

impl<'cell, X: Knotted> List<'cell, X> {
    /// Lay down `items` as the list's cells. The element type is the join of the cells' types —
    /// `Never` for an empty list — and the weight is summed in the same pass.
    pub fn new(
        writer: Writer<'cell>,
        items: impl ExactSizeIterator<Item = Value<'cell, X>>,
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'_>,
    ) -> &'cell List<'cell, X> {
        let mut items = items;
        let cells = writer.fill(items.len(), |_| {
            items
                .next()
                .expect("an exact-size iterator yields its reported length")
        });
        let ktype = list_type(types, scratch, cells.iter().map(Value::ktype));
        Self::weighed(writer, cells, ktype)
    }

    /// Lay down a key's candidates — the functions a `USING` hole or an `EVAL` offer gathers at
    /// one key — typed `List<Any>` without reading any function's type. Dispatch alone reads such
    /// a list, each function by its own type; a quantified registration's type is a scheme, which
    /// no list type joins.
    pub fn of_candidates(
        writer: Writer<'cell>,
        functions: impl ExactSizeIterator<Item = Value<'cell, X>>,
    ) -> &'cell List<'cell, X> {
        let mut functions = functions;
        let cells = writer.fill(functions.len(), |_| {
            functions
                .next()
                .expect("an exact-size iterator yields its reported length")
        });
        Self::weighed(writer, cells, KType::LIST_OF_ANY)
    }

    /// A list over value cells already resident in `writer`'s region under `ktype`, weighed as
    /// [`new`](Self::new) weighs them — a retyped data node's arm.
    pub(crate) fn weighed(
        writer: Writer<'cell>,
        cells: &'cell [Value<'cell, X>],
        ktype: KType,
    ) -> &'cell Self {
        let weight = cells.iter().fold(Weight::flat::<Self>(), |weight, cell| {
            weight.plus(cell.weight())
        });
        Self::from_run(writer, cells, ktype, weight)
    }
}

impl<'cell, X: Knotted> List<'cell, X, Link<'cell, X>> {
    /// Lay down `cells` as a knot's data node under the finished memo `ktype`, which the tie
    /// derived from the cells and the members their edges name.
    pub fn linked(writer: Writer<'cell>, cells: &[Link<'cell, X>], ktype: KType) -> &'cell Self {
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
        writer: Writer<'cell>,
        cells: &'cell [C],
        ktype: KType,
        weight: Weight,
    ) -> &'cell Self {
        resident(
            writer,
            List {
                cells,
                ktype,
                weight,
                member: PhantomData,
            },
        )
    }

    /// The same cells under `ktype` — an ascription's retype, sharing the run.
    pub fn with_type(&self, writer: Writer<'cell>, ktype: KType) -> &'cell Self {
        Self::from_run(writer, self.cells, ktype, self.weight)
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
        self.weight
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
