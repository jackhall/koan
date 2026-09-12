//! Shared test scaffolding for `memory`'s suites.

use super::substrate::{CellGraph, ReleaseAbsorption, Verdict, Writer, reattachable};

/// A continuation family for a graph whose cells only store.
struct Step;
reattachable!(Step => ());

/// Run `step` inside one cell of a one-slot graph, handing it the cell's writer.
pub(crate) fn in_cell<R>(step: impl for<'cell> FnOnce(Writer<'cell>) -> R) -> R {
    let mut graph: CellGraph<Step> = CellGraph::new(1, |_| Verdict::Pin);
    let cell = graph
        .create(None, None)
        .expect("a one-slot graph has a free slot");
    let out = graph
        .enter(cell, |context| step(context.writer()))
        .expect("a fresh cell is enterable");
    graph
        .release(cell, ReleaseAbsorption::IntoHolder)
        .expect("a cell outside its step releases");
    out
}
