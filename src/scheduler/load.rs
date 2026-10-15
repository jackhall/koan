//! The load door: a graph stood up over two slab roots, the build root entered once by the load.
//!
//! The **build root** is the build tree's one core cell, living as long as the program; its region
//! holds the root-level specialization table. The **run root** is the region every root work is
//! born under. The load runs as the build root's one step, entered here rather than through the
//! ready stack, before any root work: it is handed the build root's region doors and returns the
//! frozen type root every later step reads. See
//! [build/README.md § Steps and the drain](../build/README.md#steps-and-the-drain).

use crate::memory::{CellGraph, Header, SlabHandle, Writer};
use crate::scheduler::continuation::StepBundle;
use crate::scheduler::drain::Graph;
use crate::type_lattice::Root;
use crate::values::TypeHeader;

/// The two slab roots a loaded graph holds.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Roots {
    /// The build tree's core cell, whose region holds the root-level specialization table.
    pub build: SlabHandle,
    /// The region every root work is born under, where the top level's bindings live.
    pub run: SlabHandle,
}

/// What the load is handed: the build root's region writer and header door, at the brand of its
/// one entry. It names no handle.
pub struct LoadStep<'graph, 'here, 'step>
where
    'graph: 'here,
{
    writer: Writer<'here, 'step>,
    header: Header<'graph, 'here, 'step, TypeHeader>,
}

impl<'graph, 'here, 'step> LoadStep<'graph, 'here, 'step>
where
    'graph: 'here,
{
    /// The build root's region.
    pub fn writer(&self) -> Writer<'here, 'step> {
        self.writer
    }

    /// The build root's region header door.
    pub fn header(&self) -> Header<'graph, 'here, 'step, TypeHeader> {
        self.header
    }
}

impl<'graph, B: StepBundle<'graph>> Graph<'graph, B> {
    /// A graph over a slab of `cap` cells, at least two, holding its build root and its run root,
    /// with the build root entered once by `load`. `load` returns the frozen type root every
    /// later step reads, beside its own result; a refusal drops the graph with it.
    pub fn load<R, E>(
        cap: u32,
        load: impl FnOnce(LoadStep<'graph, '_, '_>) -> Result<(&'graph Root<'graph>, R), E>,
    ) -> Result<(Self, Roots, R), E> {
        let mut cells = CellGraph::new(cap, crate::values::verdict);
        let build = cells
            .create(None)
            .expect("a fresh slab of at least two cells admits its build root");
        let run = cells
            .create(None)
            .expect("a fresh slab of at least two cells admits its run root");
        let (type_root, out) = cells
            .enter(build, |context| {
                load(LoadStep {
                    writer: context.writer(),
                    header: context.header(),
                })
            })
            .expect("a fresh build root is enterable")?;
        Ok((
            Graph::assemble(cells, type_root, Some(build)),
            Roots { build, run },
            out,
        ))
    }
}
