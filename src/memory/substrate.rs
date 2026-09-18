//! Every cell-tier name Koan spells: `cellgraph`'s types, re-exported. This is the crate's only
//! import of `cellgraph`.
//!
//! The library takes no Koan parameter, so a name arrives here verbatim unless it carries the
//! liveness matrix's width, and the width is bound here once — [`WIDTH`] — and nowhere else.

/// The liveness matrix's width in 64-bit words: 64 slab slots, a 512-byte matrix, 8-byte masks.
pub const WIDTH: usize = 1;

pub use cellgraph::{
    Active, CellHandle, Config, CreateError, CrossedOperand, DeliverError, Delivered, Delivery,
    Dormant, DropFree, EnterError, Erased, NoDelivery, Prices, Prose, Reattachable, ReceiptError,
    RedeemError, RegisterError, ReleaseAbsorption, ReleaseError, ReleaseTenantError,
    ReleaseTreeError, Run, SlabHandle, Stale, TenantHandle, ThinRun, TreeHandle, Verdict, Writer,
    reattachable,
};

/// A carrier at rest in the region hosting it, branded by that region's `'home`; `'graph` is the
/// storage outliving the graph its value may borrow.
pub type Ready<'graph, 'home, T> = cellgraph::Ready<'graph, 'home, T, WIDTH>;

/// One operand of a placement: a ready carrier and how it crosses into the destination.
pub type Operand<'graph, 'a, 'step, V> = cellgraph::Operand<'graph, 'a, 'step, V, WIDTH>;

/// What one slot of a cell's receipt run held, drained into the step that owns the run.
pub type Receipt<'graph, 'step, 'scratch, D> =
    cellgraph::Receipt<'graph, 'step, 'scratch, D, WIDTH>;

/// The graph of cells, their regions and the liveness matrix over them, over storage `'graph` that
/// outlives it.
///
/// `S` is the family of a continuation's scratch half, and the continuation family unless named;
/// `D` is the bundle of the two families its cells deliver, which is nothing unless named.
pub type CellGraph<'graph, C, S = C, D = NoDelivery> = cellgraph::CellGraph<'graph, C, S, D, WIDTH>;

/// What a step running in a cell holds: the cell's brand `'here`, its scratch habitat's brand
/// `'scratch`, a writer at each, and the step's doors.
pub type StepContext<'graph, 'step, 'here, 'scratch, C, S = C, D = NoDelivery> =
    cellgraph::StepContext<'graph, 'step, 'here, 'scratch, C, S, D, WIDTH>;

/// Store one value in the region and hand back its resident borrow — [`Writer::fill`] at length
/// one.
pub fn resident<'cell, T: Copy>(writer: Writer<'cell>, value: T) -> &'cell T {
    &writer.fill(1, |_| value)[0]
}

/// Copy an exact-length run into the region — [`Writer::fill`] driven by the iterator, with no
/// growth path.
pub fn collect<'cell, T>(
    writer: Writer<'cell>,
    items: impl ExactSizeIterator<Item = T>,
) -> &'cell [T] {
    let mut items = items;
    writer.fill(items.len(), |_| {
        items
            .next()
            .expect("an exact-size iterator yields its reported length")
    })
}
