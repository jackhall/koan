//! Every cell-tier name Koan spells: `cellgraph`'s types, re-exported. This is the crate's only
//! import of `cellgraph`.
//!
//! The library takes no Koan parameter, so a name arrives here verbatim unless it carries the
//! liveness matrix's width, and the width is bound here once — [`WIDTH`] — and nowhere else.

/// The liveness matrix's width in 64-bit words: 128 slab slots, a 2 KiB matrix, 16-byte masks.
pub const WIDTH: usize = 2;

pub use cellgraph::{
    Active, CellHandle, CreateError, CrossedOperand, Dormant, DropFree, EnterError, Erased, Prices,
    Prose, Reattachable, RedeemError, ReleaseAbsorption, ReleaseError, ReleaseTreeError, Run,
    SlabHandle, Stale, TreeHandle, Verdict, Writer, reattachable,
};

/// A carrier at rest in the region hosting it, branded by that region's `'home`; `'graph` is the
/// storage outliving the graph its value may borrow.
pub type Ready<'graph, 'home, T> = cellgraph::Ready<'graph, 'home, T, WIDTH>;

/// One operand of a placement: a ready carrier and how it crosses into the destination.
pub type Operand<'graph, 'a, 'step, V> = cellgraph::Operand<'graph, 'a, 'step, V, WIDTH>;

/// The graph of cells, their regions and the liveness matrix over them, over storage `'graph` that
/// outlives it.
pub type CellGraph<'graph, C> = cellgraph::CellGraph<'graph, C, WIDTH>;

/// What a step running in a cell holds: the cell's brand, its writer and the step's doors.
pub type StepContext<'graph, 'step, 'here, C> =
    cellgraph::StepContext<'graph, 'step, 'here, C, WIDTH>;
