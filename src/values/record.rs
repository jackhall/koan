//! A record: field names and cells as two aligned runs in the region, sorted by symbol so a field
//! read is a binary search.

use std::marker::PhantomData;

use crate::memory::{BumpAllocator, BumpVec, Writer, resident};
use crate::parse::{BinderSymbol, Symbol};
use crate::type_lattice::{KType, TypeRegistry};

use super::{Knotted, Link, Nothing, Value, Weight, record_type};

/// An anonymous structural record value, resident in the region its cells live in. It carries no
/// nominal identity, only its fields; equality over two is blind to the order they were written in.
/// Its cells are value words, or [`Link`]s when the record is a knot's data node.
#[derive(Clone, Copy, Debug)]
pub struct Record<'graph, 'cell, X = Nothing, C = Value<'graph, 'cell, X>> {
    names: &'cell [Symbol],
    cells: &'cell [C],
    ktype: KType,
    weight: Weight,
    /// The knot member a cell's word may hold, which a link cell names only through `C`.
    member: PhantomData<Value<'graph, 'cell, X>>,
}

impl<'graph, 'cell, X: Knotted> Record<'graph, 'cell, X> {
    /// Lay down `fields`, whose names are distinct, sorted by symbol. The type is the record over
    /// each field's type in the order the fields were written; the sort and the field-type run are
    /// staged over `scratch`.
    pub fn new(
        writer: Writer<'cell>,
        fields: &[(BinderSymbol, Value<'graph, 'cell, X>)],
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'_>,
    ) -> &'cell Record<'graph, 'cell, X> {
        let order = symbol_order(fields, scratch);
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
        let ktype = record_type(
            types,
            scratch,
            fields.iter().map(|(name, cell)| (*name, cell.ktype())),
        );
        Self::from_runs(writer, names, cells, ktype, weight)
    }
}

impl<'graph, 'cell, X: Knotted> Record<'graph, 'cell, X, Link<'graph, 'cell, X>> {
    /// Lay down `fields`, whose names are distinct, sorted by symbol, as a knot's data node under
    /// the finished memo `ktype`; the sort is staged over `scratch`.
    pub fn linked(
        writer: Writer<'cell>,
        fields: &[(BinderSymbol, Link<'graph, 'cell, X>)],
        ktype: KType,
        scratch: BumpAllocator<'_>,
    ) -> &'cell Self {
        let order = symbol_order(fields, scratch);
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
        Self::from_runs(writer, names, cells, ktype, weight)
    }
}

/// The indices of `fields` in symbol order, staged over `scratch`.
fn symbol_order<'x, C>(
    fields: &[(BinderSymbol, C)],
    scratch: BumpAllocator<'x>,
) -> BumpVec<'x, usize> {
    let mut order: BumpVec<'_, usize> = BumpVec::with_capacity_in(fields.len(), scratch);
    order.extend(0..fields.len());
    order.sort_unstable_by_key(|at| fields[*at].0.symbol());
    debug_assert!(
        order
            .windows(2)
            .all(|pair| fields[pair[0]].0.symbol() != fields[pair[1]].0.symbol()),
        "a record's field names are distinct"
    );
    order
}

impl<'graph, 'cell, X: Copy, C: Copy> Record<'graph, 'cell, X, C> {
    /// A record over sorted names and aligned cells already resident in `writer`'s region, under a
    /// type and weight the caller already knows — the deep copy's arm.
    pub(crate) fn from_runs(
        writer: Writer<'cell>,
        names: &'cell [Symbol],
        cells: &'cell [C],
        ktype: KType,
        weight: Weight,
    ) -> &'cell Self {
        resident(
            writer,
            Record {
                names,
                cells,
                ktype,
                weight,
                member: PhantomData,
            },
        )
    }

    /// The same fields under `ktype` — an ascription's retype, sharing both runs.
    pub fn with_type(&self, writer: Writer<'cell>, ktype: KType) -> &'cell Self {
        Self::from_runs(writer, self.names, self.cells, ktype, self.weight)
    }

    /// The cell under `name`, found by binary search.
    pub fn field(&self, name: Symbol) -> Option<&'cell C> {
        let cells = self.cells;
        self.names.binary_search(&name).ok().map(|at| &cells[at])
    }

    /// The fields in symbol order.
    pub fn fields(
        &self,
    ) -> impl ExactSizeIterator<Item = (Symbol, &'cell C)> + use<'graph, 'cell, X, C> {
        self.names.iter().copied().zip(self.cells.iter())
    }

    pub fn names(&self) -> &'cell [Symbol] {
        self.names
    }

    pub fn cells(&self) -> &'cell [C] {
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
