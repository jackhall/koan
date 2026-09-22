//! The step bundle a koan program's steps run over.

use crate::knot::{KValue, KValueFamily};
use crate::memory::{CrossedOperand, NoScratch, Writer};
use crate::scheduler::StepBundle;

/// The state a koan program's steps run over: a cell holds a value in storage and parks nothing in
/// its scratch habitat. A placeholder: the top level replaces the value with the body runner's
/// state.
pub struct Steps;

impl<'graph> StepBundle<'graph> for Steps {
    type State = KValueFamily;
    type Scratch = NoScratch;

    fn weight<'cell>(state: &KValue<'graph, 'cell>) -> usize
    where
        'graph: 'cell,
    {
        state.weight().bytes()
    }

    fn cross<'cell>(
        writer: Writer<'cell>,
        view: &CrossedOperand<'graph, 'cell, '_, KValueFamily>,
    ) -> KValue<'graph, 'cell>
    where
        'graph: 'cell,
    {
        crate::values::cross_view(writer, view)
    }
}
