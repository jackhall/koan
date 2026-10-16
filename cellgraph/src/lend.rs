//! A cell's **lend**: the half of another cell's storage a tree cell or a tenant borrows, which a
//! step in it is handed at `'lent` ([tree/README.md § Death](tree/README.md#death)).
//!
//! A lend is named at creation ([`CellGraph::create_tree_lent`](crate::CellGraph::create_tree_lent),
//! [`CellGraph::create_tenant_lent`](crate::CellGraph::create_tenant_lent)) and resolved there into
//! a [`Lent`], which the tree or tenant pool stores on the cell and never changes. It is counted on
//! its lender by the half it lends: a scratch on the lender's scratch borrower count, which holds
//! the bump's reset off, and a region on its region borrower count, which holds the lender's
//! disposal off ([tenant.rs](tenant.rs)). A lent scratch is always the borrower's tree parent's
//! write home's, so the child count already holds that lender's disposal.
//!
//! A tree cell's count drops at its disposal, never its release: a dead cell's scratch, kept until
//! it disposes, may still name the lent bytes. A tenant has no scratch of its own; it writes its
//! host's, which outlives it. So a tenant's lend is counted only where it is a region and not its
//! host's own lend, which the host's count already holds past every reset of the host's scratch;
//! and at the tenant's release the count passes to the host's scratch, which holds one per lender
//! and drops them where it next resets. That reset also comes in a disposal walk, for a dead host
//! nothing names the scratch of, so a lender waiting on its borrower's host does not leak it.

use crate::carrier::CellHome;
use crate::handle::CellHandle;

/// What a cell borrows, named at [`CellGraph::create_tree_lent`](crate::CellGraph::create_tree_lent)
/// or [`CellGraph::create_tenant_lent`](crate::CellGraph::create_tenant_lent). A step in the cell
/// is handed the borrowed half at `'lent`: a scratch through
/// [`StepContext::lent_scratch`](crate::StepContext::lent_scratch), a region through
/// [`StepContext::lent_region`](crate::StepContext::lent_region).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lend {
    /// The scratch habitat of the tree parent's write home: what
    /// [`CellGraph::create_tree`](crate::CellGraph::create_tree) lends.
    ParentScratch,
    /// The region of the named cell, a live slab or tree cell.
    Named(CellHandle),
    /// Whatever the named live cell borrows, counted again for the new cell: a tree cell's or a
    /// tenant's own lend; a slab cell borrows nothing. A copied scratch lend is taken only where
    /// the new cell may borrow that scratch: under its lender, as
    /// [`ParentScratch`](Lend::ParentScratch) would give, or by a tenant whose host borrows it.
    Of(CellHandle),
}

/// A lend resolved at creation: which half of which write home. Stored on the tree cell or tenant
/// and never changed.
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
