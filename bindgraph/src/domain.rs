//! What the embedder supplies: the facts the graph keeps, the domain one call reads, the answer
//! function, and the bindings it reads.
//!
//! The crate decides ownership, edges and reuse, and never an answer's content. The domain comes
//! in two parts. [`Facts`] names what the graph and its instances keep: a term of a call's mapping
//! or of an `AnswersName` edge on an arm or a site, a `NameQuestion`, a `NameAnswer` and a
//! `SiteAnswer`, each `Copy` and naming no content, with each `NameQuestion`'s count of answers and
//! each answer's index below it. [`Domain`] is handed to each `instance` call and names the
//! **content** a binding holds, which may borrow that call: what a `NameQuestion` asks of
//! contents, how a term applies, the **union** of contents, the **rigid** an answered name has
//! where no answer gives it content, a content's digest and whether it lasts as long as the graph.
//! Since no kept type names a content, holding one does not compile. An [`Answers`] answers one
//! `SiteQuestion`, reaching the binding only through the [`Doors`] it is handed and making only
//! the asks its question declares, so every ask lands in the instance's admission.
//!
//! The embedder keeps these contracts, which the crate's correctness rests on:
//!
//! - **Answers are counted.** A `NameQuestion` has [`Facts::answer_count`] answers, at least one,
//!   and every answer [`Domain::ask`] gives has an index below it.
//! - **A digest is its content.** Two contents with one [`Domain::digest`] are one content, and a
//!   content that [`Domain::lasts`] stays readable for the graph's life.
//! - **Applying never grows unreported.** [`Domain::apply`]'s result is no deeper than the
//!   term's `Built` depth plus the deepest content it substitutes. A part of a caller's name,
//!   such as an element solved out of a list, is reported [`Nesting::Bare`], and where `apply`
//!   cannot compute the part, over open content for one, it returns `None` and leaves the callee's
//!   name open. Without that, a self-call taking a part of its own name nests forever. A union is
//!   no deeper than its deepest alternative, and holds each alternative once, a union's
//!   alternatives among them, so a recursion that adds an alternative already there repeats.
//! - **The empty union is no alternative.** Applying a term with a name it mentions only as an
//!   alternative bound to `union(&[])` leaves that alternative out. Where no alternative is
//!   left, it gives `union(&[])` itself, so its digest tells the crate nothing is left.
//! - **Answering is deterministic.** The same answers to the same asks in the same order give the
//!   same asks and the same `SiteAnswer`.
//! - **Reads name what an answer depends on.** A question's reads are every name its answer
//!   depends on. An earlier answer reaches it through an answered name, which it reads as any
//!   other name, and the crate computes the site names behind it.
//! - **Open names and rigids stay apart.** A name left open inside passed content stands for its
//!   own unknown, even where the callee binds a name with the same id. So does each occurrence of
//!   a rigid: no judgement relates two. An ask over several names answers as for unrelated
//!   unknowns where one of them is open or a rigid.
//!
//! See [the crate's README](../README.md#the-domains-contract).

use std::collections::BTreeMap;
use std::fmt;
use std::hash::{Hash, Hasher};

use crate::graph::Ask;
use crate::id::{NameId, QuestionId};

/// How a term mentions one of the names it is over.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum Nesting {
    /// The name itself, or a part of it.
    Bare,
    /// Inside a type the mapping builds.
    Built,
    /// One alternative of a union the term builds: the name itself, beside the union's other
    /// alternatives. A union adds no depth.
    Alternative,
}

/// What the graph and its instances keep: the text's terms and the asks and answers an instance
/// is made of, each `Copy` and naming no content.
pub trait Facts {
    /// What a call maps a callee's name to, and what an `AnswersName` edge on an arm or a site
    /// gives an answered name: a term over names, each left open where it appears.
    type Term: Copy + Eq + fmt::Debug;
    /// One judgement about one or more bound names.
    type NameQuestion: Copy + Eq + fmt::Debug;
    /// A `NameQuestion`'s answer.
    type NameAnswer: Copy + Eq + fmt::Debug;
    /// One `SiteQuestion` answered under a binding.
    type SiteAnswer: Copy + PartialEq + fmt::Debug;

    /// Each name `term` mentions: bare, inside a type it builds, or as one alternative of a union
    /// it builds.
    fn mentions(&self, term: Self::Term) -> Vec<(NameId, Nesting)>;

    /// The arm an answer takes, if it decides one.
    fn arm(&self, answer: Self::SiteAnswer) -> Option<u32>;

    /// How many answers `question` has: at least one.
    fn answer_count(&self, question: Self::NameQuestion) -> u32;

    /// `answer`'s index among `question`'s answers, below [`Self::answer_count`].
    fn answer_index(&self, question: Self::NameQuestion, answer: Self::NameAnswer) -> u32;

    /// The answer at `index` among `question`'s answers: [`Self::answer_index`]'s inverse.
    fn nth_answer(&self, question: Self::NameQuestion, index: u32) -> Self::NameAnswer;
}

/// What one `instance` call reads: the content a binding holds, and the judgements over it.
pub trait Domain: Sized {
    /// The facts the graph this domain reads is laid over.
    type Facts: Facts;
    /// What a name is bound to: a term that may mention names, each left open where it appears.
    /// It may borrow the call it is read in.
    type Content: Copy + Eq + Hash + fmt::Debug;

    /// The answer to `question` about names bound to `contents`, in the ask's order, each `None`
    /// where its name is left open.
    fn ask(
        &self,
        question: NameQuestion<Self>,
        contents: &[Option<Self::Content>],
    ) -> NameAnswer<Self>;

    /// `term` with each name it mentions replaced by that name's content in `binding`; a name
    /// `binding` leaves open stays. `None` leaves the callee's name open, or the answered name its
    /// rigid, and is owed wherever a part of the content cannot be computed: the result is never
    /// deeper than the term's `Built` depth plus the deepest content substituted.
    fn apply(&self, term: Term<Self>, binding: &Binding<Self>) -> Option<Self::Content>;

    /// The union of `alternatives`: a content holding each once, a union's alternatives among
    /// them. The union of none is the **empty union**, which applying leaves out wherever a name
    /// bound to it is one alternative of a union.
    fn union(&self, alternatives: &[Self::Content]) -> Self::Content;

    /// The content of answered `name` where no answer gives it one: one term per name, the same
    /// in every binding, standing for whatever the run binds there.
    fn rigid(&self, name: NameId) -> Self::Content;

    /// `content`'s digest: two contents with one digest are one content.
    fn digest(&self, content: Self::Content) -> u128;

    /// Whether `content` stays readable for the graph's life.
    fn lasts(&self, content: Self::Content) -> bool;
}

/// A domain's term.
pub(crate) type Term<D> = <<D as Domain>::Facts as Facts>::Term;
/// A domain's `NameQuestion`.
pub(crate) type NameQuestion<D> = <<D as Domain>::Facts as Facts>::NameQuestion;
/// A domain's `NameAnswer`.
pub(crate) type NameAnswer<D> = <<D as Domain>::Facts as Facts>::NameAnswer;

/// The embedder's answer function.
pub trait Answers<F: Facts> {
    /// Answers `question`, reading the binding only through `doors`. Must be deterministic: the
    /// same answers to the same asks in the same order give the same asks and the same answer.
    fn answer(&mut self, question: QuestionId, doors: &mut Doors<'_, F>) -> F::SiteAnswer;
}

/// The content each name is bound to; a name the binding leaves out is open. A call's input,
/// never kept.
pub struct Binding<D: Domain> {
    names: BTreeMap<NameId, D::Content>,
}

impl<D: Domain> Binding<D> {
    /// The binding that leaves every name open.
    pub fn new() -> Self {
        Self {
            names: BTreeMap::new(),
        }
    }

    /// Binds `name` to `content`, replacing what it held.
    pub fn bind(&mut self, name: NameId, content: D::Content) {
        self.names.insert(name, content);
    }

    /// What `name` is bound to, or `None` where it is open.
    pub fn get(&self, name: NameId) -> Option<D::Content> {
        self.names.get(&name).copied()
    }

    /// The bound names and their content, in `NameId` order.
    pub fn iter(&self) -> impl Iterator<Item = (NameId, D::Content)> {
        self.names.iter().map(|(name, content)| (*name, *content))
    }

    /// The binding with only the names `keep` admits.
    pub(crate) fn filtered(&self, mut keep: impl FnMut(NameId) -> bool) -> Self {
        let names = self.names.iter().filter(|(name, _)| keep(**name));
        Self {
            names: names.map(|(name, content)| (*name, *content)).collect(),
        }
    }
}

impl<D: Domain> Default for Binding<D> {
    fn default() -> Self {
        Self::new()
    }
}

impl<D: Domain> Clone for Binding<D> {
    fn clone(&self) -> Self {
        Self {
            names: self.names.clone(),
        }
    }
}

impl<D: Domain> PartialEq for Binding<D> {
    fn eq(&self, other: &Self) -> bool {
        self.names == other.names
    }
}

impl<D: Domain> Eq for Binding<D> {}

impl<D: Domain> Hash for Binding<D> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.names.hash(state);
    }
}

impl<D: Domain> fmt::Debug for Binding<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.names.iter()).finish()
    }
}

/// The one way an answer function reaches the binding: each ask goes through here, checked
/// against the asks its question declares, and is served by whatever the caller supplies — the
/// domain over a binding, or a handle's fields in a replay.
pub struct Doors<'a, F: Facts> {
    question: QuestionId,
    reads: &'a [NameId],
    asks: &'a [Ask<F>],
    serve: &'a mut dyn FnMut(usize) -> F::NameAnswer,
}

impl<'a, F: Facts> Doors<'a, F> {
    /// Doors for answering `question`, which reads `reads` and declares `asks`; `serve` answers
    /// the declared ask at a position.
    pub(crate) fn new(
        question: QuestionId,
        reads: &'a [NameId],
        asks: &'a [Ask<F>],
        serve: &'a mut dyn FnMut(usize) -> F::NameAnswer,
    ) -> Self {
        Self {
            question,
            reads,
            asks,
            serve,
        }
    }

    /// Asks `question` of `names`, in order, and returns the answer.
    ///
    /// # Panics
    ///
    /// Where a name is not among the reads the text declared for the question being answered, or
    /// the question does not declare this ask: the description and the answer function disagree.
    pub fn ask(&mut self, names: &[NameId], question: F::NameQuestion) -> F::NameAnswer {
        for name in names {
            assert!(
                self.reads.contains(name),
                "{:?} asked about {name:?}, which it does not read",
                self.question,
            );
        }
        let declared = self
            .asks
            .iter()
            .position(|ask| ask.names() == names && ask.question() == question);
        let Some(declared) = declared else {
            panic!(
                "{:?} makes an ask it does not declare: {question:?} of {names:?}",
                self.question
            );
        };
        (self.serve)(declared)
    }
}
