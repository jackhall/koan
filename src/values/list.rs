//! A list: one run of cells in the region, typed by the join of its elements. Its cells are value
//! words, or [`Link`]s when the list is a knot's data node.

use std::marker::PhantomData;

use crate::memory::{BumpAllocator, Writer, resident};
use crate::type_lattice::{KType, TypeRegistry, join};

use super::{Knotted, Link, Nothing, Value, Weight};

/// A list value, resident in the region its cells live in.
#[derive(Clone, Copy, Debug)]
pub struct List<'graph, 'cell, X = Nothing, C = Value<'graph, 'cell, X>> {
    cells: &'cell [C],
    ktype: KType,
    weight: Weight,
    /// The knot member a cell's word may hold, which a link cell names only through `C`.
    member: PhantomData<Value<'graph, 'cell, X>>,
}

impl<'graph, 'cell, X: Knotted> List<'graph, 'cell, X> {
    /// Lay down `items` as the list's cells. The element type is the join of the cells' types —
    /// `Never` for an empty list — and the weight is summed in the same pass.
    pub fn new(
        writer: Writer<'cell>,
        items: impl ExactSizeIterator<Item = Value<'graph, 'cell, X>>,
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'_>,
    ) -> &'cell List<'graph, 'cell, X> {
        let mut items = items;
        let mut element = KType::NEVER;
        let mut weight = Weight::flat::<Self>();
        let cells = writer.fill(items.len(), |_| {
            let cell = items
                .next()
                .expect("an exact-size iterator yields its reported length");
            element = join(types, scratch, element, cell.ktype());
            weight = weight.plus(cell.weight());
            cell
        });
        Self::from_run(writer, cells, types.list(element), weight)
    }
}

impl<'graph, 'cell, X: Knotted> List<'graph, 'cell, X, Link<'graph, 'cell, X>> {
    /// Lay down `cells` as a knot's data node under the finished memo `ktype`, which the tie
    /// derived from the cells and the members their edges name.
    pub fn linked(
        writer: Writer<'cell>,
        cells: &[Link<'graph, 'cell, X>],
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

impl<'graph, 'cell, X: Copy, C: Copy> List<'graph, 'cell, X, C> {
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

    pub fn cells(&self) -> &'cell [C] {
        self.cells
    }

    pub fn get(&self, index: usize) -> Option<&'cell C> {
        self.cells.get(index)
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
