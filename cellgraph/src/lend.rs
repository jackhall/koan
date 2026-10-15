//! A tree cell's **lend**: the half of another cell's storage it borrows from its creation to its
//! disposal, which a step in it, or in a tenant of it, is handed at `'lent`
//! ([tree/README.md § Death](tree/README.md#death)).
//!
//! A lend is named at creation ([`CellGraph::create_tree_lent`](crate::CellGraph::create_tree_lent))
//! and resolved there into a [`Lent`], which the tree pool stores on the cell and never changes. It
//! is counted on its lender by the half it lends: a scratch on the lender's scratch borrower count,
//! which holds the bump's reset off, and a region on its region borrower count, which holds the
//! lender's disposal off ([tenant.rs](tenant.rs)). A lent scratch is always the borrower's tree
//! parent's write home's, so the child count already holds that lender's disposal. Both counts drop
//! at the borrower's disposal, never its release: a dead cell's scratch, kept until it disposes,
//! may still name the lent bytes.

use crate::carrier::CellHome;
use crate::handle::CellHandle;

/// What a tree cell borrows from its creation to its disposal, named at
/// [`CellGraph::create_tree_lent`](crate::CellGraph::create_tree_lent). A step in the cell, or in
/// a tenant of it, is handed the borrowed half at `'lent`: a scratch through
/// [`StepContext::lent_scratch`](crate::StepContext::lent_scratch), a region through
/// [`StepContext::lent_region`](crate::StepContext::lent_region).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lend {
    /// The scratch habitat of the tree parent's write home: what
    /// [`CellGraph::create_tree`](crate::CellGraph::create_tree) lends.
    ParentScratch,
    /// The region of the named cell, a live slab or tree cell.
    Named(CellHandle),
    /// Whatever the named live cell borrows, counted again for the new cell. A tenant's is its
    /// host's; a slab cell borrows nothing. A copied scratch lend is taken only under its lender,
    /// as [`ParentScratch`](Lend::ParentScratch) would give.
    Of(CellHandle),
}

/// A lend resolved at creation: which half of which write home. Stored on the tree cell and never
/// changed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Lent {
    Scratch(CellHome),
    Region(CellHome),
}

impl Lent {
    /// The write home whose half is borrowed.
    pub(crate) fn lender(self) -> CellHome {
        match self {
            Lent::Scratch(home) | Lent::Region(home) => home,
        }
    }
}
