//! A list: one run of cells in the region, typed by the join of its elements.

use crate::memory::{BumpAllocator, Writer};
use crate::type_lattice::{KType, TypeRegistry, join};

use super::{Value, Weight, resident};

/// A list value, resident in the region its cells live in.
#[derive(Clone, Copy, Debug)]
pub struct List<'graph, 'cell> {
    cells: &'cell [Value<'graph, 'cell>],
    ktype: KType,
    weight: Weight,
}

impl<'graph, 'cell> List<'graph, 'cell> {
    /// Lay down `items` as the list's cells. The element type is the join of the cells' types —
    /// `Never` for an empty list — and the weight is summed in the same pass.
    pub fn new(
        writer: Writer<'cell>,
        items: impl ExactSizeIterator<Item = Value<'graph, 'cell>>,
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'_>,
    ) -> &'cell List<'graph, 'cell> {
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

    /// A list over cells already resident in `writer`'s region, under a type and weight the caller
    /// already knows — the deep copy's arm.
    pub(crate) fn from_run(
        writer: Writer<'cell>,
        cells: &'cell [Value<'graph, 'cell>],
        ktype: KType,
        weight: Weight,
    ) -> &'cell List<'graph, 'cell> {
        resident(
            writer,
            List {
                cells,
                ktype,
                weight,
            },
        )
    }

    /// The same cells under `ktype` — an ascription's retype, sharing the run.
    pub fn with_type(&self, writer: Writer<'cell>, ktype: KType) -> &'cell List<'graph, 'cell> {
        Self::from_run(writer, self.cells, ktype, self.weight)
    }

    pub fn cells(&self) -> &'cell [Value<'graph, 'cell>] {
        self.cells
    }

    pub fn get(&self, index: usize) -> Option<&'cell Value<'graph, 'cell>> {
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
