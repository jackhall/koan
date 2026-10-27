//! The load door: a graph stood up over its run root, after the load.
//!
//! The **run root** is the slab cell every root work is born under. The load runs here, outside
//! the graph and before any root work, and returns the type tiers and the facts every later step
//! reads; what it writes rests in program storage. The settlements frames run under are the graph's
//! [build arena](super::arena), not cells. See
//! [build/README.md § Steps and the drain](../build/README.md#steps-and-the-drain).

use crate::memory::SlabHandle;
use crate::scheduler::continuation::StepBundle;
use crate::scheduler::drain::Graph;
use crate::scheduler::seek::Settles;
use crate::scope::LoadFacts;
use crate::values::{BuildBudget, TypeTiers};

impl<'graph, B: StepBundle<'graph>> Graph<'graph, B> {
    /// Run `load`, then stand up a graph over a slab of `cap` cells holding its run root, its
    /// steps' types and facts read over the tiers and the facts `load` returns beside its own
    /// result, its misses settled through the door it returns beside them, its unheld settlements
    /// within `budget`. A refusal stands up no graph.
    #[allow(clippy::type_complexity)]
    pub fn load<R, E>(
        cap: u32,
        budget: BuildBudget,
        load: impl FnOnce() -> Result<
            (
                TypeTiers<'graph>,
                (&'graph LoadFacts<'graph>, &'graph dyn Settles<'graph>),
                R,
            ),
            E,
        >,
    ) -> Result<(Self, SlabHandle, R), E> {
        let (tiers, facts, out) = load()?;
        let mut graph = Graph::over(cap, tiers, facts, budget);
        let run = graph
            .root()
            .expect("a fresh slab of at least one cell admits its run root");
        Ok((graph, run, out))
    }
}
