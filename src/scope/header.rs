//! The region header: what a koan graph keeps per habitat half, one record for the run tree and
//! the build tree alike ([the build module's design](../build/README.md#references-between-the-trees)).
//!
//! A half's branch of the type lattice rests here, laid by the first door that interns into the
//! half. A build cell's region also keeps the fixed table of its specializations, laid with the
//! cell, and a specialization's region its home, which the drain sets once the cell has enlisted,
//! and the facts its resumption settled, which its own step sets as it enlists. A frame reads
//! them through the region it borrows, so what it reads of the program's shape stays shared in
//! program storage.
//!
//! The record is `scope`'s, so it can hold what `scope` names; `values` reads the branch alone,
//! through the projection [`RegionHeader`] implements ([`Projects`]), and names no other member.

use std::cell::Cell;
use std::num::NonZeroUsize;

use crate::memory::{LaysRuns, Projects, Writer, reattachable};
use crate::type_lattice::Branch;
use crate::values::{BranchCell, BranchSlot, SpecHome, SpecTable};

use super::settled::Settled;

/// The header family a koan graph keeps per habitat half: a [`HalfHeader`].
pub struct RegionHeader;

reattachable!(RegionHeader => &'cell HalfHeader<'cell>);

impl<'graph> Projects<'graph> for RegionHeader {
    type Projection = BranchCell;

    fn lay<'cell>(writer: Writer<'cell, '_>) -> &'cell HalfHeader<'cell>
    where
        'graph: 'cell,
    {
        &writer.fill(1, |_| HalfHeader {
            branch: Cell::new(None),
            table: Cell::new(None),
            home: Cell::new(None),
            settled: Cell::new(None),
        })[0]
    }

    fn project<'cell>(header: &'cell HalfHeader<'cell>) -> BranchSlot<'cell>
    where
        'graph: 'cell,
    {
        &header.branch
    }
}

/// What a habitat half keeps in its header: its branch of the type lattice; for a build cell's
/// region, the table of its specializations; and for a specialization's region, its home and the
/// facts its resumption settled.
pub struct HalfHeader<'cell> {
    branch: Cell<Option<&'cell Branch<'cell>>>,
    table: Cell<Option<&'cell SpecTable<'cell>>>,
    home: Cell<Option<SpecHome>>,
    settled: Cell<Option<&'cell Settled<'cell>>>,
}

impl<'cell> HalfHeader<'cell> {
    /// The half's branch of the type lattice, once a door has interned into the half.
    pub fn branch(&self) -> Option<&'cell Branch<'cell>> {
        self.branch.get()
    }

    /// The half's specialization table, for a build cell's region.
    pub fn table(&self) -> Option<&'cell SpecTable<'cell>> {
        self.table.get()
    }

    /// Lay the half's specialization table through `lay`, `length` entries long, its children's
    /// tables `nested` long. A header that has a table keeps it, and the call returns it.
    pub fn lay_table(
        &self,
        lay: impl LaysRuns<'cell>,
        length: NonZeroUsize,
        nested: NonZeroUsize,
    ) -> &'cell SpecTable<'cell> {
        if let Some(table) = self.table.get() {
            return table;
        }
        let table = SpecTable::lay(lay, length, nested);
        self.table.set(Some(table));
        table
    }

    /// The specialization whose region this header is, and its parent, once the drain has set it.
    pub fn home(&self) -> Option<SpecHome> {
        self.home.get()
    }

    /// Record that this header's region is the specialization `home` names.
    pub fn set_home(&self, home: SpecHome) {
        self.home.set(Some(home));
    }

    /// The facts the specialization whose region this header is settled, once its step enlisted
    /// with them.
    pub fn settled(&self) -> Option<&'cell Settled<'cell>> {
        self.settled.get()
    }

    /// Record the facts this header's specialization settled. Only `Step::enlist` calls it: a
    /// specialization's own step, which ends there and never runs again, so the slot is set at
    /// most once.
    pub(crate) fn settle(&self, settled: &'cell Settled<'cell>) {
        self.settled.set(Some(settled));
    }
}
