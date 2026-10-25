//! The region header: what a koan graph keeps per habitat half.
//!
//! A half's branch of the type lattice rests here, laid by the first door that interns into the
//! half, beside the [decision memo](super::memo) of the cells whose results are built in it. The record is `scope`'s, so it can hold what `scope` names; `values` reads the branch
//! through the projection [`RegionHeader`] implements ([`Projects`]).

use std::cell::Cell;

use crate::memory::{Projects, Writer, reattachable};
use crate::type_lattice::Branch;
use crate::values::{BranchCell, BranchSlot};

use super::memo::DecisionMemo;

/// How many entries one half lays in its decision memo before it only reads it: a chain that
/// misses at every hop stops growing the region its results are built in.
const MEMO_BUDGET: u32 = 32;

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
            memo: Cell::new(None),
            memo_laid: Cell::new(0),
        })[0]
    }

    fn project<'cell>(header: &'cell HalfHeader<'cell>) -> BranchSlot<'cell>
    where
        'graph: 'cell,
    {
        &header.branch
    }
}

/// What a habitat half keeps in its header: its branch of the type lattice, and the decision memo
/// of the cells whose results are built in it, beside how many entries it has laid.
pub struct HalfHeader<'cell> {
    branch: Cell<Option<&'cell Branch<'cell>>>,
    memo: Cell<Option<&'cell DecisionMemo<'cell>>>,
    memo_laid: Cell<u32>,
}

impl<'cell> HalfHeader<'cell> {
    /// The half's branch of the type lattice, once a door has interned into the half.
    pub fn branch(&self) -> Option<&'cell Branch<'cell>> {
        self.branch.get()
    }

    /// The decision memo of the cells whose results are built in this half, once one has decided.
    pub fn memo(&self) -> Option<&'cell DecisionMemo<'cell>> {
        self.memo.get()
    }

    /// Whether the half may lay another memo entry.
    pub fn memo_room(&self) -> bool {
        self.memo_laid.get() < MEMO_BUDGET
    }

    /// Make `memo` the half's: one it `laid` an entry into, or one seeded from another half's.
    pub fn set_memo(&self, memo: &'cell DecisionMemo<'cell>, laid: bool) {
        self.memo.set(Some(memo));
        self.memo_laid.set(self.memo_laid.get() + u32::from(laid));
    }
}
