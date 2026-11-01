//! The region header: what a koan graph keeps per habitat half.
//!
//! A half's branch of the type lattice rests here, laid by the first door that interns into the
//! half, beside the [decision memo](super::memo) of the cells whose results are built in it, and,
//! in a kept cell's region, the outermost [link](super::fault) of the fault it ended with. The
//! record is `scope`'s, so it can hold what `scope` names; `values` reads the branch through the
//! projection [`RegionHeader`] implements ([`Projects`]).

use std::cell::Cell;
use std::marker::PhantomData;

use crate::memory::{Projects, Writer, reattachable};
use crate::type_lattice::Branch;
use crate::values::{BranchCell, BranchSlot, KnottedFamily, Value};

use super::fault::Link;
use super::memo::DecisionMemo;

/// How many entries one half lays in its decision memo before it only reads it: a chain that
/// misses at every hop stops growing the region its results are built in.
const MEMO_BUDGET: u32 = 32;

/// The header family a koan graph keeps per habitat half: a [`HalfHeader`] whose fault holds
/// values closed over the knot members of `XF`.
pub struct RegionHeader<XF>(PhantomData<XF>);

reattachable!(RegionHeader<XF: KnottedFamily<'graph>> => &'cell HalfHeader<'cell, Value<'cell, XF::Closed<'cell>>>);

impl<'graph, XF: KnottedFamily<'graph>> Projects<'graph> for RegionHeader<XF> {
    type Projection = BranchCell;

    fn lay<'cell>(
        writer: Writer<'cell, '_>,
    ) -> &'cell HalfHeader<'cell, Value<'cell, XF::Closed<'cell>>>
    where
        'graph: 'cell,
    {
        &writer.fill(1, |_| HalfHeader {
            branch: Cell::new(None),
            memo: Cell::new(None),
            memo_laid: Cell::new(0),
            link: Cell::new(None),
        })[0]
    }

    fn project<'cell>(
        header: &'cell HalfHeader<'cell, Value<'cell, XF::Closed<'cell>>>,
    ) -> BranchSlot<'cell>
    where
        'graph: 'cell,
    {
        &header.branch
    }
}

/// What a habitat half keeps in its header: its branch of the type lattice, the decision memo of
/// the cells whose results are built in it, beside how many entries it has laid, and, in a kept
/// cell's region, the outermost link of the fault it ended with, whose values are `V`s.
pub struct HalfHeader<'cell, V> {
    branch: Cell<Option<&'cell Branch<'cell>>>,
    memo: Cell<Option<&'cell DecisionMemo<'cell>>>,
    memo_laid: Cell<u32>,
    link: Cell<Option<&'cell Link<'cell, V>>>,
}

impl<'cell, V> HalfHeader<'cell, V> {
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

    /// The outermost link of the fault a kept cell ended with, once it has.
    pub fn link(&self) -> Option<&'cell Link<'cell, V>> {
        self.link.get()
    }

    /// Record `link`, the outermost link of the fault the cell ends with.
    pub fn set_link(&self, link: &'cell Link<'cell, V>) {
        self.link.set(Some(link));
    }

    /// Make `memo` the half's: one it `laid` an entry into, or one seeded from another half's.
    pub fn set_memo(&self, memo: &'cell DecisionMemo<'cell>, laid: bool) {
        self.memo.set(Some(memo));
        self.memo_laid.set(self.memo_laid.get() + u32::from(laid));
    }
}
