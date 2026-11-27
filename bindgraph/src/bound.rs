//! Bound instances: a node's answers under a binding, named by a handle that is the instance's
//! admission, filed by view, and resting in bumps the embedder lends.
//!
//! A [`Bound`] borrows a laid [`Graph`] and its [`Facts`], and keeps no content: a [`Domain`] is
//! handed to each [`Bound::instance`] call, and its content lives for that call
//! ([evaluate.rs](bound/evaluate.rs)). An instance is a function of its node and its
//! **admission**, the asks its answers made with the answers they took, so a [`Handle`], the
//! admission written in the node's fields, never dangles and names no storage.
//!
//! `instance` looks the view up and evaluates on a miss. A view rests by the digests of its
//! content: kept for the life of the bump lent to the `Bound` where every content lasts, and
//! otherwise in a fixed table of its node's own. An instance's answers rest in the [`Frozen`] part
//! the `Bound` was laid over, or in a [`Resting`] store the embedder lends each call
//! ([store.rs](bound/store.rs)); a read through a handle whose answers rest nowhere replays the
//! answer function, the doors serving the handle's fields, and asks the domain nothing.
//! [`Bound::answer`] reads one question's answer the same way, and [`Bound::content`] derives an
//! answered name's content again from its node's view, as a caller takes a name from a callee.
//!
//! The load is a `Bound` over [`Frozen::empty`] with no view-table limit, and a `Resting` with no
//! budget over the same bump; [`Bound::freeze`] makes the frozen part every run's `Bound` is laid
//! over.
//!
//! See [the crate's README](../README.md#instances).

mod evaluate;
mod store;

use std::cell::Cell;

use bumpstorage::{Bump, BumpAllocator, DigestTable, FrozenTable, LaysRuns};

use crate::domain::{Answers, Binding, Domain, Doors, Facts, Kept};
use crate::graph::Graph;
use crate::id::{NameId, NodeId, QuestionId};
use evaluate::Session;
pub use store::Resting;
use store::{AnswerEntry, ViewEntry, ViewTable, answer_word, view_word};

/// A handle: its instance's admission, packed into the words a caller holds. Two bindings of a
/// node get equal handles exactly where their admissions are equal.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct Handle<'h> {
    node: NodeId,
    words: &'h [u64],
}

impl<'h> Handle<'h> {
    /// The handle of `node` whose admission an `instance` call wrote as `words`: what a caller
    /// holding the words hands back to read the instance.
    pub fn new(node: NodeId, words: &'h [u64]) -> Self {
        Self { node, words }
    }

    /// The node the instance is of.
    pub fn node(self) -> NodeId {
        self.node
    }

    /// The admission's words: one field per declared ask of the node.
    pub fn words(self) -> &'h [u64] {
        self.words
    }
}

/// What a `Bound` made before its freeze: every answer rested and every view filed. Read-only,
/// `Sync` where the facts are, never freed while its bump lives, and shared by every `Bound` laid
/// over it.
pub struct Frozen<'f, K: Kept> {
    answers: FrozenTable<'f, AnswerEntry<'f, K>>,
    views: FrozenTable<'f, ViewEntry<'f>>,
}

impl<K: Kept> Frozen<'_, K> {
    /// The frozen part that holds nothing: what the load is laid over.
    pub const fn empty() -> Self {
        Self {
            answers: FrozenTable::EMPTY,
            views: FrozenTable::EMPTY,
        }
    }
}

/// How often a `Bound` asked the domain and ran the answer function, the measure of what keeping
/// answers saves, and what its search for knots stacked and closed.
#[cfg(feature = "perf")]
#[derive(Copy, Clone, PartialEq, Eq, Default, Debug)]
pub struct Counts {
    /// Asks the domain answered.
    pub asks: u64,
    /// Runs of the answer function, one per question answered.
    pub runs: u64,
    /// Demands put on Tarjan's stack: those whose name or question lies in a text component.
    pub stacked: u64,
    /// Knots closed whose every member takes or is a union alternative: those that carry one type.
    pub knots_of_one_type: u64,
    /// Knots closed of any other kind.
    pub other_knots: u64,
}

/// A laid graph read under bindings, with its views filed and the frozen part it is laid over.
pub struct Bound<'g, 'f, 'a, F: Facts> {
    graph: &'g Graph<F::Kept>,
    facts: &'g F,
    frozen: &'f Frozen<'f, F::Kept>,
    /// The bump lent for the `Bound`'s life: its kept views and view tables.
    lay: BumpAllocator<'a>,
    /// The views whose every content lasts, or every view where `view_slots` is `None`.
    views: DigestTable<'a, ViewEntry<'a>>,
    /// Per node, its table of views that do not last, laid on the first.
    tables: &'a [Cell<Option<&'a ViewTable<'a>>>],
    /// The slots of each node's view table; `None` keeps every view.
    view_slots: Option<usize>,
    walked: u64,
    #[cfg(feature = "perf")]
    counts: Counts,
}

impl<'g, 'f, 'a, F: Facts> Bound<'g, 'f, 'a, F> {
    /// Instances of `graph` under `facts`, over the frozen part `frozen`, keeping its views in
    /// `lend` for its life. Each node's views that do not last rest in a table of `view_slots`
    /// slots; `None` keeps every view, and `Some(0)` keeps none of them.
    pub fn new(
        graph: &'g Graph<F::Kept>,
        facts: &'g F,
        frozen: &'f Frozen<'f, F::Kept>,
        lend: &'a Bump,
        view_slots: Option<usize>,
    ) -> Self {
        Self {
            graph,
            facts,
            frozen,
            lay: lend,
            views: DigestTable::new(lend),
            tables: lend.lay(graph.nodes().len(), |_| Cell::new(None)),
            view_slots,
            walked: 0,
            #[cfg(feature = "perf")]
            counts: Counts::default(),
        }
    }

    /// The handle of `node`'s instance under `binding`, written into `into`. Looks the view up,
    /// and evaluates on a miss: answers the node's questions, gives the answered names the binding
    /// leaves out their content, and follows each call it takes, finding or making each callee's
    /// evaluation transitively. A name the binding types is taken as given, answered names
    /// included. What the evaluations make rests in `resting`, within its budget.
    ///
    /// # Panics
    ///
    /// Where `into` is not the node's [`handle_width`](crate::BindingNode::handle_width) words, or
    /// an answer takes an arm its question does not lay.
    pub fn instance<'h, D: Domain<Facts = F>>(
        &mut self,
        resting: &Resting<'_, F::Kept>,
        node: NodeId,
        binding: &Binding<D>,
        domain: &D,
        answers: &mut impl Answers<F>,
        into: &'h mut [u64],
    ) -> Handle<'h> {
        let width = self.graph.node(node).handle_width();
        assert_eq!(
            into.len(),
            width,
            "a handle of {node:?} takes {width} words"
        );
        let key = self.view_key(node, binding, domain);
        let word = view_word(node, &key);
        if !self.filed(node, &key, word, into) {
            let session = Session::new(self, resting, domain, answers);
            into.copy_from_slice(&session.evaluate(node, binding, key, word));
        }
        Handle { node, words: into }
    }

    /// Copies `handle`'s answers into `into`, one per question its node owns, in `owns` order:
    /// from where they rest, or by replaying the answer function with the doors serving the
    /// handle's fields. A replay reads no binding and asks the domain nothing; its answers rest
    /// again where `resting` has room.
    ///
    /// # Panics
    ///
    /// Where the answer function asks what the instance did not, or leaves an ask it made unasked:
    /// it is not deterministic.
    pub fn answers(
        &mut self,
        resting: &Resting<'_, F::Kept>,
        handle: Handle<'_>,
        answers: &mut impl Answers<F>,
        into: &mut Vec<<F::Kept as Kept>::SiteAnswer>,
    ) {
        self.read(resting, handle.node, handle.words, answers, into);
    }

    /// `handle`'s answer to `question`, which its node owns: from where the node's answers rest,
    /// or by replaying the answer function for `question` alone, the doors serving the handle's
    /// fields. Equals `question`'s element of [`Self::answers`], and allocates nothing.
    ///
    /// # Panics
    ///
    /// Where `handle`'s node does not own `question`, or the replay asks what the instance did not.
    pub fn answer(
        &mut self,
        resting: &Resting<'_, F::Kept>,
        handle: Handle<'_>,
        question: QuestionId,
        answers: &mut impl Answers<F>,
    ) -> <F::Kept as Kept>::SiteAnswer {
        let Handle { node, words } = handle;
        let owns = self.graph.node(node).owns();
        let Ok(position) = owns.binary_search(&question) else {
            panic!("{node:?} does not own {question:?}");
        };
        match self.rested(resting, node, words) {
            Some(rested) => rested[position],
            None => self.replay(node, words, question, answers, &mut |_| {}),
        }
    }

    /// The content of answered `name`, which `node` binds, under `binding`: derived from `node`'s
    /// view of `binding`, its evaluation found filed or made, as a caller derives a name it takes
    /// from a callee. A name `binding` types is taken as given. What a made evaluation makes
    /// rests in `resting` and files its view, as `instance` would.
    ///
    /// # Panics
    ///
    /// Where `name` is not an answered name of `node`.
    pub fn content<D: Domain<Facts = F>>(
        &mut self,
        resting: &Resting<'_, F::Kept>,
        node: NodeId,
        binding: &Binding<D>,
        name: NameId,
        domain: &D,
        answers: &mut impl Answers<F>,
    ) -> D::Content {
        let answered = self.graph.node(node).answered();
        assert!(
            answered.binary_search(&name).is_ok(),
            "{name:?} is no answered name of {node:?}"
        );
        if let Some(content) = binding.get(name) {
            return content;
        }
        Session::new(self, resting, domain, answers).content(node, binding, name)
    }

    /// Lays the frozen part in the bump lent to this `Bound`: the entries of the part it was laid
    /// over and of `resting`, and every view it files. Nothing is copied but the views in its
    /// tables.
    pub fn freeze(self, resting: &'a Resting<'a, F::Kept>) -> &'a Frozen<'a, F::Kept>
    where
        'f: 'a,
    {
        let base: &'a Frozen<'a, F::Kept> = self.frozen;
        let answers = base.answers.words().chain(resting.entries());
        let answers = FrozenTable::placed_at(self.lay, base.answers.len() + resting.len(), answers);
        let mut views: Vec<(u64, &'a ViewEntry<'a>)> = base.views.words().collect();
        views.extend(self.views.words());
        for (index, table) in self.tables.iter().enumerate() {
            if let Some(table) = table.get() {
                views.extend(table.entries(NodeId::new(index), self.lay));
            }
        }
        let views = FrozenTable::placed_at(self.lay, views.len(), views);
        let mut frozen = Some(Frozen { answers, views });
        &self.lay.lay(1, |_| frozen.take().expect("one frozen part"))[0]
    }

    /// How many edges the evaluations have walked: each call followed, and each call a result
    /// was taken through. A repeated `instance` call adds none.
    pub fn walked(&self) -> u64 {
        self.walked
    }

    /// The graph these instances are of.
    pub fn graph(&self) -> &'g Graph<F::Kept> {
        self.graph
    }

    /// What the `Bound` has counted: asks, runs, demands put on Tarjan's stack, and knots closed
    /// by kind.
    #[cfg(feature = "perf")]
    pub fn counts(&self) -> Counts {
        self.counts
    }

    /// The digests of `binding`'s content at the names `node` can read.
    fn view_key<D: Domain<Facts = F>>(
        &self,
        node: NodeId,
        binding: &Binding<D>,
        domain: &D,
    ) -> Vec<Option<u128>> {
        let relevant = self.graph.relevant(node).iter();
        let digest = |name| binding.get(name).map(|content| domain.digest(content));
        relevant.map(|&name| digest(name)).collect()
    }

    /// The handle words filed under `node`'s view of `binding`, if it is filed. Makes nothing.
    #[cfg(test)]
    pub(crate) fn filed_view<D: Domain<Facts = F>>(
        &self,
        node: NodeId,
        binding: &Binding<D>,
        domain: &D,
    ) -> Option<Vec<u64>> {
        let key = self.view_key(node, binding, domain);
        let mut words = vec![0; self.graph.node(node).handle_width()];
        self.filed(node, &key, view_word(node, &key), &mut words)
            .then_some(words)
    }

    /// Copies the handle words filed under `node`'s view `key`, homed at `word`, into `into`;
    /// whether the view is filed.
    fn filed(&self, node: NodeId, key: &[Option<u128>], word: u64, into: &mut [u64]) -> bool {
        let entry = (self.frozen.views.find(word, |entry| entry.is(node, key)))
            .or_else(|| self.views.find(word, |entry| entry.is(node, key)));
        if let Some(entry) = entry {
            into.copy_from_slice(entry.words());
            return true;
        }
        let table = self.tables[node.index()].get();
        table.is_some_and(|table| table.get(word, key, into))
    }

    /// Files `node`'s handle `words` under its view `key`, homed at `word`: kept where every
    /// content lasts or no table limit is set, and otherwise in the node's table.
    fn file(&self, node: NodeId, key: &[Option<u128>], word: u64, lasting: bool, words: &[u64]) {
        match self.view_slots {
            Some(0) if !lasting => {}
            Some(slots) if !lasting => {
                let cell = &self.tables[node.index()];
                let table = cell.get().unwrap_or_else(|| {
                    let width = self.graph.node(node).handle_width();
                    let relevant = self.graph.relevant(node).len();
                    let table = ViewTable::laid(self.lay, slots, relevant, width);
                    cell.set(Some(table));
                    table
                });
                table.put(word, key, words);
            }
            _ => {
                if self.views.find(word, |entry| entry.is(node, key)).is_none() {
                    let entry = ViewEntry::laid(self.lay, node, key, words);
                    self.views.insert_at(self.lay, word, entry);
                }
            }
        }
    }

    /// Rests `node`'s `answers` under its handle `words` in `resting`, where neither the frozen
    /// part nor `resting` holds them and the budget has room.
    fn keep(
        &self,
        resting: &Resting<'_, F::Kept>,
        node: NodeId,
        words: &[u64],
        answers: &[<F::Kept as Kept>::SiteAnswer],
    ) {
        let word = answer_word(node, words);
        let frozen = self
            .frozen
            .answers
            .find(word, |entry| entry.is(node, words));
        if frozen.is_none() && resting.find(word, node, words).is_none() {
            resting.rest(word, node, words, answers);
        }
    }

    /// The answers of `node`'s handle `words`, where they rest: in the frozen part or `resting`.
    fn rested<'r>(
        &self,
        resting: &'r Resting<'_, F::Kept>,
        node: NodeId,
        words: &[u64],
    ) -> Option<&'r [<F::Kept as Kept>::SiteAnswer]>
    where
        'f: 'r,
    {
        let word = answer_word(node, words);
        let frozen = self
            .frozen
            .answers
            .find(word, |entry| entry.is(node, words));
        let rested = frozen.or_else(|| resting.find(word, node, words));
        rested.map(|entry| entry.answers())
    }

    /// Replays the answer function for `question`, which `node` owns, the doors serving the
    /// handle `words`' fields, and marks each field it reads through `read`.
    fn replay(
        &mut self,
        node: NodeId,
        words: &[u64],
        question: QuestionId,
        answers: &mut dyn Answers<F>,
        read: &mut dyn FnMut(usize),
    ) -> <F::Kept as Kept>::SiteAnswer {
        let binding_node = self.graph.node(node);
        let text = self.graph.question(question);
        let facts = self.facts;
        let mut serve = |declared: usize| {
            let field = text.fields[declared] as usize;
            let held = binding_node.fields[field].read(words);
            assert!(
                held != 0,
                "the answer function is not deterministic: {question:?} asked what its \
                 instance did not"
            );
            read(field);
            facts.nth_answer(text.asks()[declared].question(), held as u32 - 1)
        };
        let mut doors = Doors::new(
            question,
            text.reads(),
            text.asks(),
            &mut serve,
            Default::default(),
        );
        let answer = answers.answer(question, &mut doors);
        #[cfg(feature = "perf")]
        {
            self.counts.runs += 1;
        }
        answer
    }

    /// Copies the answers of `node`'s handle `words` into `into`: from where they rest, or by a
    /// replay, whose answers rest again.
    fn read(
        &mut self,
        resting: &Resting<'_, F::Kept>,
        node: NodeId,
        words: &[u64],
        answers: &mut dyn Answers<F>,
        into: &mut Vec<<F::Kept as Kept>::SiteAnswer>,
    ) {
        into.clear();
        if let Some(rested) = self.rested(resting, node, words) {
            into.extend_from_slice(rested);
            return;
        }
        let binding_node = self.graph.node(node);
        let mut read = vec![false; binding_node.fields.len()];
        for &question in binding_node.owns() {
            let answer = self.replay(node, words, question, answers, &mut |field| {
                read[field] = true
            });
            into.push(answer);
        }
        let binding_node = self.graph.node(node);
        for (field, read) in binding_node.fields.iter().zip(read) {
            assert!(
                read || field.read(words) == 0,
                "the answer function is not deterministic: {node:?}'s questions left unasked an \
                 ask its instance made"
            );
        }
        resting.rest(answer_word(node, words), node, words, into);
    }
}
