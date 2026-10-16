//! A record: field names and cells as two aligned runs in the region, sorted by symbol so a field
//! read is a binary search.

use std::cell::Cell;
use std::marker::PhantomData;

use crate::memory::{BumpAllocator, BumpVec, Writer, resident};
use crate::symbols::{BinderSymbol, Symbol};
use crate::type_lattice::KType;

use super::settled::{composite_bits, restamp_bits};
use super::{Chain, Knotted, Link, MemoBits, Nothing, Value, Weight, record_type};

/// An anonymous structural record value, resident in the region its cells live in. It carries no
/// nominal identity, only its fields; equality over two is blind to the order they were written in.
/// Its cells are value words, or [`Link`]s when the record is a knot's data node.
#[derive(Debug)]
pub struct Record<'cell, X = Nothing, C = Value<'cell, X>> {
    names: &'cell [Symbol],
    cells: &'cell [C],
    ktype: KType,
    weight: Cell<Weight>,
    /// The knot member a cell's word may hold, which a link cell names only through `C`.
    member: PhantomData<Value<'cell, X>>,
}

impl<'cell, X: Knotted> Record<'cell, X> {
    /// Lay down `fields`, whose names are distinct, sorted by symbol. The type is the record over
    /// each field's type in the order the fields were written; the sort and the field-type run are
    /// staged over `scratch`.
    pub fn new(
        writer: Writer<'cell, '_>,
        fields: &[(BinderSymbol, Value<'cell, X>)],
        types: &Chain<'_, '_, 'cell, '_, '_>,
        scratch: BumpAllocator<'_>,
    ) -> &'cell Record<'cell, X> {
        let ktype = record_type(
            types,
            scratch,
            fields
                .iter()
                .map(|(name, cell)| (*name, cell.concrete_ktype())),
        );
        Self::typed(writer, fields, ktype, types, scratch)
    }

    /// Lay down `fields` as [`new`](Self::new) does, under `ktype`, a type the caller knows they
    /// satisfy — a view's rebuilt record, at the type its slot declares — so no type is interned.
    pub fn typed(
        writer: Writer<'cell, '_>,
        fields: &[(BinderSymbol, Value<'cell, X>)],
        ktype: KType,
        types: &Chain<'_, '_, 'cell, '_, '_>,
        scratch: BumpAllocator<'_>,
    ) -> &'cell Record<'cell, X> {
        let order = symbol_order(fields, scratch);
        let names = writer.fill(order.len(), |at| fields[order[at]].0.symbol());
        let cells = writer.fill(order.len(), |at| fields[order[at]].1);
        Self::weighed(writer, names, cells, ktype, types)
    }

    /// A record over sorted names and aligned value cells already resident in `writer`'s region
    /// under `ktype`, weighed as [`new`](Self::new) weighs them — a retyped data node's arm.
    pub(crate) fn weighed(
        writer: Writer<'cell, '_>,
        names: &'cell [Symbol],
        cells: &'cell [Value<'cell, X>],
        ktype: KType,
        types: &Chain<'_, '_, 'cell, '_, '_>,
    ) -> &'cell Self {
        let weight = Weight::flat::<Self>().plus(Weight::run::<Symbol>(names.len()));
        let weight = cells
            .iter()
            .fold(weight, |weight, cell| weight.plus(cell.weight()));
        let bits = composite_bits(types, ktype, cells);
        Self::from_runs(writer, names, cells, ktype, weight.with_bits(bits))
    }
}

impl<'cell, X: Knotted> Record<'cell, X, Link<'cell, X>> {
    /// Lay down `fields`, whose names are distinct, sorted by symbol, as a knot's data node under
    /// the finished memo `ktype`; the sort is staged over `scratch`.
    pub fn linked(
        writer: Writer<'cell, '_>,
        fields: &[(BinderSymbol, Link<'cell, X>)],
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

impl<'cell, X: Copy, C: Copy> Record<'cell, X, C> {
    /// A record over sorted names and aligned cells already resident in `writer`'s region, under a
    /// type and weight the caller already knows — the deep copy's arm.
    pub(crate) fn from_runs(
        writer: Writer<'cell, '_>,
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
                weight: Cell::new(weight),
                member: PhantomData,
            },
        )
    }

    /// The same fields under `ktype` — an ascription's retype, sharing both runs, its bits
    /// recomputed from the new memo.
    pub fn with_type(
        &self,
        writer: Writer<'cell, '_>,
        ktype: KType,
        types: &Chain<'_, '_, 'cell, '_, '_>,
    ) -> &'cell Self {
        let weight = self.weight.get();
        let weight = weight.with_bits(restamp_bits(weight.bits(), types, ktype));
        Self::from_runs(writer, self.names, self.cells, ktype, weight)
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
}

/// The runs a read outside `values` reaches only through [the door](super::surface).
impl<'cell, X: Copy> Record<'cell, X> {
    pub(super) fn names(&self) -> &'cell [Symbol] {
        self.names
    }

    pub(super) fn cells(&self) -> &'cell [Value<'cell, X>] {
        self.cells
    }
}

/// A knot's data node's runs, which the knot layer ties and reads.
impl<'cell, X: Copy> Record<'cell, X, Link<'cell, X>> {
    /// The cell under `name`, found by binary search.
    pub fn field(&self, name: Symbol) -> Option<&'cell Link<'cell, X>> {
        let cells = self.cells;
        self.names.binary_search(&name).ok().map(|at| &cells[at])
    }

    pub fn names(&self) -> &'cell [Symbol] {
        self.names
    }

    pub fn cells(&self) -> &'cell [Link<'cell, X>] {
        self.cells
    }
}
