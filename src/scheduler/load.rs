//! The load door: a graph stood up over two slab roots, the build root entered once by the load.
//!
//! The **build root** is the build tree's one core cell, living as long as the program; its region
//! holds the root-level specialization table. The **run root** is the region every root work is
//! born under. The load runs in the build root's one entry, here rather than through the ready
//! stack, before any root work, and returns the frozen type root every later step reads; what it
//! writes rests in program storage. The entry lays the build root's header and its table, as long
//! as the root bound, once the load returns. See
//! [build/README.md § Steps and the drain](../build/README.md#steps-and-the-drain).

use crate::memory::{CellGraph, Projects as _, SlabHandle};
use crate::scheduler::continuation::ContinuationFamily;
use crate::scheduler::continuation::StepBundle;
use crate::scheduler::delivery::KDelivery;
use crate::scheduler::drain::Graph;
use crate::type_lattice::Root;
use crate::values::{Half, SpecBounds, TypeHeader};

/// The two slab roots a loaded graph holds.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Roots {
    /// The build tree's core cell, whose region holds the root-level specialization table.
    pub build: SlabHandle,
    /// The region every root work is born under, where the top level's bindings live.
    pub run: SlabHandle,
}

impl<'graph, B: StepBundle<'graph>> Graph<'graph, B> {
    /// A graph over a slab of `cap` cells, at least two, holding its build root and its run root,
    /// with the build root entered once to run `load`. `load` returns the frozen type root every
    /// later step reads, beside its own result; a refusal drops the graph with it. The build root's
    /// table holds `bounds.root` children, and every specialization's `bounds.nested`.
    pub fn load<R, E>(
        cap: u32,
        bounds: SpecBounds,
        load: impl FnOnce() -> Result<(&'graph Root<'graph>, R), E>,
    ) -> Result<(Self, Roots, R), E> {
        let mut cells: CellGraph<'graph, ContinuationFamily<B>, B::Scratch, KDelivery, TypeHeader> =
            CellGraph::new(cap, crate::values::verdict);
        let build = cells
            .create(None)
            .expect("a fresh slab of at least two cells admits its build root");
        let run = cells
            .create(None)
            .expect("a fresh slab of at least two cells admits its run root");
        let (type_root, out) = cells
            .enter(build, |context| {
                let loaded = load()?;
                let half = Half::of(context.writer(), context.header());
                let header = context
                    .header()
                    .get_or_lay(|| TypeHeader::lay(context.writer()));
                header.lay_table(half, bounds.root, bounds.nested);
                Ok(loaded)
            })
            .expect("a fresh build root is enterable")?;
        Ok((
            Graph::assemble(cells, type_root, Some(build)),
            Roots { build, run },
            out,
        ))
    }
}
