//! A record: field names and cells as two aligned runs in the region, sorted by symbol so a field
//! read is a binary search.

use crate::memory::{BumpAllocator, BumpVec, Writer};
use crate::parse::{BinderSymbol, Symbol};
use crate::type_lattice::{KType, TypeRegistry};

use super::{Value, Weight, resident};

/// An anonymous structural record value, resident in the region its cells live in. It carries no
/// nominal identity, only its fields; equality over two is blind to the order they were written in.
#[derive(Clone, Copy, Debug)]
pub struct Record<'graph, 'cell> {
    names: &'cell [Symbol],
    cells: &'cell [Value<'graph, 'cell>],
    ktype: KType,
    weight: Weight,
}

impl<'graph, 'cell> Record<'graph, 'cell> {
    /// Lay down `fields`, whose names are distinct, sorted by symbol. The type is the record over
    /// each field's type in the order the fields were written; the sort and the field-type run are
    /// staged over `scratch`.
    pub fn new(
        writer: Writer<'cell>,
        fields: &[(BinderSymbol, Value<'graph, 'cell>)],
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'_>,
    ) -> &'cell Record<'graph, 'cell> {
        let mut order: BumpVec<'_, usize> = BumpVec::with_capacity_in(fields.len(), scratch);
        order.extend(0..fields.len());
        order.sort_unstable_by_key(|at| fields[*at].0.symbol());
        debug_assert!(
            order
                .windows(2)
                .all(|pair| fields[pair[0]].0.symbol() != fields[pair[1]].0.symbol()),
            "a record's field names are distinct"
        );
        let mut field_types: BumpVec<'_, (BinderSymbol, KType)> =
            BumpVec::with_capacity_in(fields.len(), scratch);
        field_types.extend(fields.iter().map(|(name, cell)| (*name, cell.ktype())));
        let mut weight = Weight::flat::<Self>();
        let names = writer.fill(order.len(), |at| {
            weight = weight.plus(Weight::flat::<Symbol>());
            fields[order[at]].0.symbol()
        });
        let cells = writer.fill(order.len(), |at| {
            let cell = fields[order[at]].1;
            weight = weight.plus(cell.weight());
            cell
        });
        Self::from_runs(
            writer,
            names,
            cells,
            types.record(scratch, &field_types),
            weight,
        )
    }

    /// A record over sorted names and aligned cells already resident in `writer`'s region, under a
    /// type and weight the caller already knows — the deep copy's arm.
    pub(crate) fn from_runs(
        writer: Writer<'cell>,
        names: &'cell [Symbol],
        cells: &'cell [Value<'graph, 'cell>],
        ktype: KType,
        weight: Weight,
    ) -> &'cell Record<'graph, 'cell> {
        resident(
            writer,
            Record {
                names,
                cells,
                ktype,
                weight,
            },
        )
    }

    /// The same fields under `ktype` — an ascription's retype, sharing both runs.
    pub fn with_type(&self, writer: Writer<'cell>, ktype: KType) -> &'cell Record<'graph, 'cell> {
        Self::from_runs(writer, self.names, self.cells, ktype, self.weight)
    }

    /// The cell under `name`, found by binary search.
    pub fn field(&self, name: Symbol) -> Option<&'cell Value<'graph, 'cell>> {
        let cells = self.cells;
        self.names.binary_search(&name).ok().map(|at| &cells[at])
    }

    /// The fields in symbol order.
    pub fn fields(
        &self,
    ) -> impl ExactSizeIterator<Item = (Symbol, &'cell Value<'graph, 'cell>)> + use<'graph, 'cell>
    {
        self.names.iter().copied().zip(self.cells.iter())
    }

    pub fn names(&self) -> &'cell [Symbol] {
        self.names
    }

    pub fn cells(&self) -> &'cell [Value<'graph, 'cell>] {
        self.cells
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    pub fn ktype(&self) -> KType {
        self.ktype
    }

    pub fn weight(&self) -> Weight {
        self.weight
    }
}
