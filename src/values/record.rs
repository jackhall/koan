//! A record: field names and cells as two aligned runs in the region, sorted by symbol so a field
//! read is a binary search.

use std::marker::PhantomData;

use crate::memory::{BumpAllocator, BumpVec, Writer, resident};
use crate::symbols::{BinderSymbol, Symbol};
use crate::type_lattice::{KType, TypeRegistry};

use super::digest::{ContentDigest, DigestHasher, Digests, Tag, composite};
use super::{Knotted, Link, Nothing, Value, Weight, record_type};

/// An anonymous structural record value, resident in the region its cells live in. It carries no
/// nominal identity, only its fields; equality over two is blind to the order they were written in.
/// Its cells are value words, or [`Link`]s when the record is a knot's data node.
#[derive(Clone, Copy, Debug)]
pub struct Record<'cell, X = Nothing, C = Value<'cell, X>> {
    names: &'cell [Symbol],
    cells: &'cell [C],
    ktype: KType,
    weight: Weight,
    /// The knot member a cell's word may hold, which a link cell names only through `C`.
    member: PhantomData<Value<'cell, X>>,
}

impl<'cell, X: Knotted> Record<'cell, X> {
    /// Lay down `fields`, whose names are distinct, sorted by symbol. The type is the record over
    /// each field's type in the order the fields were written; the sort and the field-type run are
    /// staged over `scratch`.
    pub fn new(
        writer: Writer<'cell>,
        fields: &[(BinderSymbol, Value<'cell, X>)],
        types: &TypeRegistry<'_>,
        scratch: BumpAllocator<'_>,
    ) -> &'cell Record<'cell, X> {
        let order = symbol_order(fields, scratch);
        let names = writer.fill(order.len(), |at| fields[order[at]].0.symbol());
        let cells = writer.fill(order.len(), |at| fields[order[at]].1);
        let ktype = record_type(
            types,
            scratch,
            fields
                .iter()
                .map(|(name, cell)| (*name, cell.concrete_ktype())),
        );
        Self::weighed(writer, names, cells, ktype)
    }

    /// A record over sorted names and aligned value cells already resident in `writer`'s region
    /// under `ktype`, weighed as [`new`](Self::new) weighs them — a retyped data node's arm.
    pub(crate) fn weighed(
        writer: Writer<'cell>,
        names: &'cell [Symbol],
        cells: &'cell [Value<'cell, X>],
        ktype: KType,
    ) -> &'cell Self {
        let weight = Weight::flat::<Self>().plus(Weight::run::<Symbol>(names.len()));
        let weight = cells
            .iter()
            .fold(weight, |weight, cell| weight.plus(cell.weight()));
        Self::from_runs(writer, names, cells, ktype, weight)
    }

    /// The record's content digest: its type and each field's name and cell, in symbol order, so
    /// field order is blind, through `memo`.
    pub fn digest(&self, memo: &mut Digests) -> ContentDigest {
        memo.memo(self, |memo| {
            let mut hasher = DigestHasher::new(Tag::Contents);
            hasher.count(self.names.len());
            for (name, cell) in self.names.iter().zip(self.cells) {
                hasher.feed(name).digest(cell.digest_in(memo));
            }
            composite(Tag::Record, self.ktype, hasher.finished())
        })
    }
}

impl<'cell, X: Knotted> Record<'cell, X, Link<'cell, X>> {
    /// Lay down `fields`, whose names are distinct, sorted by symbol, as a knot's data node under
    /// the finished memo `ktype`; the sort is staged over `scratch`.
    pub fn linked(
        writer: Writer<'cell>,
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

    pub fn ktype(&self) -> KType {
        self.ktype
    }

    pub fn weight(&self) -> Weight {
        self.weight
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
