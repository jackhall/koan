//! The region header: what a koan graph keeps per habitat half.
//!
//! A half's branch of the type lattice rests here, laid by the first door that interns into the
//! half. The record is `scope`'s, so it can hold what `scope` names; `values` reads the branch
//! through the projection [`RegionHeader`] implements ([`Projects`]).

use std::cell::Cell;

use crate::memory::{Projects, Writer, reattachable};
use crate::type_lattice::Branch;
use crate::values::{BranchCell, BranchSlot};

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
        })[0]
    }

    fn project<'cell>(header: &'cell HalfHeader<'cell>) -> BranchSlot<'cell>
    where
        'graph: 'cell,
    {
        &header.branch
    }
}

/// What a habitat half keeps in its header: its branch of the type lattice.
pub struct HalfHeader<'cell> {
    branch: Cell<Option<&'cell Branch<'cell>>>,
}

impl<'cell> HalfHeader<'cell> {
    /// The half's branch of the type lattice, once a door has interned into the half.
    pub fn branch(&self) -> Option<&'cell Branch<'cell>> {
        self.branch.get()
    }
}
