//! The cases' and laws' handle on a `Bound`: the kept load, handles read back as answers, and an
//! answer function that counts its runs.
//!
//! [`Kept`] is the load's shape: a `Bound` over the empty frozen part with no view-table limit,
//! and a `Resting` with no budget over the same bump, so every view is filed and every answer
//! rests. Most cases and laws read through it.

use bumpstorage::Bump;

use super::toy::{Toy, ToyAnswers, Verdict};
use crate::{Answers, Binding, Bound, Doors, Frozen, Graph, Handle, NodeId, QuestionId, Resting};

/// The frozen part that holds nothing, which the load is laid over.
pub(super) static EMPTY: Frozen<'static, Toy> = Frozen::empty();

/// An answer function that records each question it runs for, around the toy's.
pub(super) struct Counting<'a> {
    pub(super) inner: &'a mut ToyAnswers,
    pub(super) runs: Vec<QuestionId>,
}

impl<'a> Counting<'a> {
    pub(super) fn new(inner: &'a mut ToyAnswers) -> Self {
        Self {
            inner,
            runs: Vec::new(),
        }
    }
}

impl Answers<Toy> for Counting<'_> {
    fn answer(&mut self, question: QuestionId, doors: &mut Doors<'_, Toy>) -> Verdict {
        self.runs.push(question);
        self.inner.answer(question, doors)
    }
}

/// A load: a `Bound` keeping every view, and a `Resting` keeping every answer, in one bump.
pub(super) struct Kept<'g, 'b> {
    pub(super) bound: Bound<'g, 'static, 'b, Toy>,
    pub(super) resting: &'b Resting<'b, Toy>,
}

impl<'g, 'b> Kept<'g, 'b> {
    pub(super) fn new(graph: &'g Graph<Toy>, bump: &'b Bump) -> Self {
        Self {
            bound: Bound::new(graph, &Toy, &EMPTY, bump, None),
            resting: Resting::new(bump, None),
        }
    }

    /// The handle words of `node`'s instance under `binding`.
    pub(super) fn instance(
        &mut self,
        node: NodeId,
        binding: &Binding<Toy>,
        answers: &mut impl Answers<Toy>,
    ) -> Vec<u64> {
        handle(&mut self.bound, self.resting, node, binding, answers)
    }

    /// The answers `node`'s handle `words` reads.
    pub(super) fn read(
        &mut self,
        node: NodeId,
        words: &[u64],
        answers: &mut impl Answers<Toy>,
    ) -> Vec<Verdict> {
        read(&mut self.bound, self.resting, node, words, answers)
    }

    /// The answers of `node`'s instance under `binding`.
    pub(super) fn answers(
        &mut self,
        node: NodeId,
        binding: &Binding<Toy>,
        answers: &mut impl Answers<Toy>,
    ) -> Vec<Verdict> {
        let words = self.instance(node, binding, answers);
        self.read(node, &words, answers)
    }

    /// The answers of `node`'s view of `binding`, where it is filed. Makes nothing.
    pub(super) fn found(
        &mut self,
        node: NodeId,
        binding: &Binding<Toy>,
        answers: &mut impl Answers<Toy>,
    ) -> Option<Vec<Verdict>> {
        let words = self.bound.filed_view(node, binding, &Toy)?;
        Some(self.read(node, &words, answers))
    }

    /// How many instances' answers rest.
    pub(super) fn resting(&self) -> usize {
        self.resting.len()
    }

    /// How many instances of `node` rest.
    pub(super) fn resting_at(&self, node: NodeId) -> usize {
        self.resting.nodes().filter(|&at| at == node).count()
    }
}

/// The handle words of `node`'s instance under `binding`, in `bound` over `resting`.
pub(super) fn handle(
    bound: &mut Bound<'_, '_, '_, Toy>,
    resting: &Resting<'_, Toy>,
    node: NodeId,
    binding: &Binding<Toy>,
    answers: &mut impl Answers<Toy>,
) -> Vec<u64> {
    let mut words = vec![0; bound.graph().node(node).handle_width()];
    bound.instance(resting, node, binding, &Toy, answers, &mut words);
    words
}

/// The answers `node`'s handle `words` reads, in `bound` over `resting`.
pub(super) fn read(
    bound: &mut Bound<'_, '_, '_, Toy>,
    resting: &Resting<'_, Toy>,
    node: NodeId,
    words: &[u64],
    answers: &mut impl Answers<Toy>,
) -> Vec<Verdict> {
    let mut read = Vec::new();
    bound.answers(resting, Handle::new(node, words), answers, &mut read);
    read
}
