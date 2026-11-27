//! The **fold to the binding graph's text**: the load's records read as the description the
//! binding graph is laid from ([the binding graph](../../design/binding-graph.md),
//! [the crate](../../bindgraph/README.md)), and the facts the graph keeps beside it
//! ([`GraphFacts`]).
//!
//! - **Bodies** are the bodies that own units: the program, each callable and each quote's code,
//!   nested as their shapes are. A body's statements are its units in order, then one for its
//!   closing record. Before a unit's statement come those the run binds first: a statement of its
//!   own for each part the unit holds in place ([`held`]), inner first then left to right, each
//!   after the inline bodies evaluating it runs; then the inline bodies the unit runs itself. An
//!   inline body — a block, a `MODULE` body — runs in its encloser, and is laid there as an owning
//!   body is: a statement for its entry, then its units', each after what it holds. So no
//!   statement binds two units' names, and no holder's arrival reads a name its own statement
//!   binds.
//! - **Site names** are the unknowns a question or a call's mapping reads: a type name by its
//!   level, read off the chain of the body that reads it, and a module binder's two names
//!   ([`NameKind`]), the type its module carries — by its [`Carried`](DerivedTerm::Carried) term,
//!   or as a path class's recipe reads it — and its content. Each binds where the unknown binds.
//!   Any other derived name but a decided one is no name: a question reading it reads the names
//!   its term is written over. A path class's variable is no name either: a question reading it
//!   reads the names its recipe does, the content name only where the class reads the binder's
//!   identity, under a view or any other tree over the binder.
//! - **Answered names** are the decided derived names ([`DerivedTerm::Decided`]), one per segment
//!   reading it, since its types are written in that segment's levels: its deciding question is
//!   laid at the segment's closing, and an `AnswersName` edge on each of its arms gives the name
//!   that arm's type. One whose question and arms read no name the chain binds is constant, and
//!   one with an arm written over a path class, which no term spells, is read through its
//!   question and arms.
//! - **Questions** are the stored site questions whose decision some binding may change: each ask
//!   with no answer at every binding is a `NameQuestion` over the names it reads on the question's
//!   chain. A question all of whose needed asks hold one answer at every binding of its chain is
//!   decided by the text, and is laid as no question. One reading a name no binding writes in — a
//!   level no body homes — is left out, and counted ([`GraphFacts::left`]): the load lays none.
//! - **Calls**: each candidate of a use's selection question passes its callee what its laid
//!   mapping says, on the candidate's arm; a call the text selects, where the question is decided
//!   by the text or the walk traced the use, passes it at the call's site.
//! - **Holders** ([`held`]): a `LET` binder holding a call's value whole binds at its statement.
//!   Where an answer through the call gives its type, its arrival — a callable result's answered
//!   name or closed term — is what an onward call gives it, so the instance that follows the call
//!   makes the holder's too.
//! - A body's entry is **open to callers** outside the text where its name escapes, where it is a
//!   quote's code, and where an `EVAL` offers it to the code it runs. Composed code's own body is
//!   one, inside a root that binds nothing.
//!
//! [`Layout`] places each node's handle in its body's slab, the words a frame of the body holds, and
//! [`Instances`] is the agent's: the crate's `Bound` over the program's graph, which every step
//! borrows to make and read the instances its frames reach.
//!
//! [`GraphAnswers`] is the answer function: the site question's own [`decide`], each ask a binding
//! may change asked through the crate's doors and every other answered as at the load; and a
//! deciding question's ask, its answer's index the arm.

mod held;

use bindgraph::{
    Answers, BodyId, Doors, EdgeId, Facts, Kept, NameId, Nesting, NodeId, QuestionId,
    Site as GraphSite, Text,
};

use crate::memory::{BumpAllocator, BumpBackedMap, BumpVec, Lays, bump_table, collect};
use crate::scope::{
    Address, Answer, Asks, Called, Content, Decider, DerivedName, DerivedTerm, Fixed, Leaf,
    LoadFacts, NameAddress, NodeCapture, Passing, PathEntry, Payload, PointAsk, Question,
    Resolution, Resolved, Rule, Run, Segment, Selectable, ShapeKind, Shapes, Site, SiteAnswer,
    SiteQuestion, SiteRule, Slot, Tree, UnitIndex, Unknown, Within, decide, free_variables,
};
use crate::source::SourceRef;
use crate::type_lattice::{
    DeclaredType, DerivedId, Interval, KKind, KType, Parametric, PathClass, TypeNode, Variable,
};
use crate::values::{Chain, ContentDigest, retype_target};

/// Where a name stands in the types written over it: a site name's lexical level, or a derived
/// name by its term's id.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NameVariable {
    Level(usize),
    Derived(DerivedId),
}

impl NameVariable {
    /// The name `variable` is, where it is one.
    pub fn of(variable: Variable) -> Option<NameVariable> {
        match variable {
            Variable::Lexical { level, .. } => Some(NameVariable::Level(level)),
            Variable::Derived { term, .. } => Some(NameVariable::Derived(term)),
            _ => None,
        }
    }
}

/// A stored site question's key: its body, its site and its rule.
pub type SiteKey = (Segment, crate::scope::Site, SiteRule);

/// A term of the graph's text, by its index among the fold's.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TermId(u32);

/// A `NameQuestion`: one ask of a stored site question, by its index among the fold's.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct AskId(u32);

/// Which of the names an unknown binds a name of the graph is: a type name, or one of a module
/// binder's two names, the type its module carries and its content.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum NameKind {
    Type,
    Carried,
    Content,
}

/// One name of the graph: its address, which of the names there it is, and its declared bound.
/// The variable it is in the types written over it is the reader's: a capture crossing into a
/// quote's code is the name its source reads, spelled by a variable of the code's own.
#[derive(Clone, Copy, Debug)]
pub struct GraphName {
    pub address: NameAddress,
    pub kind: NameKind,
    pub upper: KType,
}

/// What a call maps a callee's name to.
#[derive(Clone, Copy, Debug)]
pub enum GraphTerm<'g> {
    /// A type written over names, beside each name it mentions and how, and the variable each
    /// such name is in it.
    Type {
        ty: DeclaredType<Parametric>,
        mentions: &'g [(NameId, Nesting)],
        variables: &'g [(NameId, NameVariable)],
    },
    /// A name of the caller's, passed on bare.
    Name(NameId),
    /// The type a module the text fixes carries into its callee, or one an argument at a parameter
    /// declared `Any` carries: its carried name's. Its ends are written over names as a `Type`
    /// term is; where they close to one type, that is the carried content a frame binds.
    Carried {
        typed: Interval,
        mentions: &'g [(NameId, Nesting)],
        variables: &'g [(NameId, NameVariable)],
    },
    /// A module the text fixes, by its knot member's digest, which indexes its tree: its content
    /// name's. Its bare signature and sealing are read off the tree at `site` of `segment`, where
    /// it is passed.
    Module {
        content: ContentDigest,
        segment: Segment,
        site: Site,
    },
}

/// One ask of a question of `segment`'s, over the names it reads in order, beside the variable
/// each is in its question, by its name's index among `names`: a name a path class's recipe reads
/// is in no variable.
#[derive(Clone, Copy, Debug)]
pub struct GraphAsk<'g> {
    pub question: Question<'g>,
    pub segment: Segment,
    pub names: &'g [NameId],
    pub variables: &'g [(u16, NameVariable)],
}

/// One question of the graph: a stored site question, or the question deciding an answered name.
#[derive(Clone, Copy, Debug)]
pub enum GraphQuestion<'g> {
    /// A stored site question, beside each of its asks' `NameQuestion`, `None` for an ask whose
    /// answer holds at every binding.
    Site {
        question: &'g SiteQuestion<'g>,
        asks: &'g [Option<AskId>],
    },
    /// A decided derived name's question, its answer's index the arm whose type the name takes:
    /// its ask, `None` where it reads no name a binding the question's instance sees writes in.
    Decides {
        question: Question<'g>,
        ask: Option<AskId>,
    },
}

impl<'g> GraphQuestion<'g> {
    /// The stored site question, where it is one.
    pub fn stored(&self) -> Option<&'g SiteQuestion<'g>> {
        match *self {
            GraphQuestion::Site { question, .. } => Some(question),
            GraphQuestion::Decides { .. } => None,
        }
    }
}

/// What the graph of one base keeps: its names, terms, asks and questions, by the ids its text
/// hands the crate.
#[derive(Clone, Copy)]
pub struct GraphFacts<'g> {
    pub names: &'g [GraphName],
    pub terms: &'g [GraphTerm<'g>],
    pub asks: &'g [GraphAsk<'g>],
    pub questions: &'g [GraphQuestion<'g>],
    /// Each stored site question's graph question, by the question's key, in key order.
    pub sites: &'g [(SiteKey, QuestionId)],
    /// Each call, by its edge's id: its callee and where it is written.
    pub calls: &'g [(BodyId, Option<SourceRef>)],
    /// One path of each path class the load resolved, beside its resolution — what the class reads
    /// — in class order.
    pub classes: &'g [(PathClass, PathEntry<'g>, Resolution<'g>)],
    /// How many stored questions the fold left out, each reading a name no binding writes in.
    pub left: usize,
    /// Each owning body's id, by segment: `None` for an inline body, which runs in its encloser.
    pub bodies: &'g [Option<BodyId>],
    /// Each part held in place, by its segment and site, beside the statement of its own it binds
    /// at, before its unit's ([`held`]).
    pub held: &'g [((Segment, Site), GraphSite)],
    /// The owning body and the unit's run index of each statement laid before that unit's own, a
    /// part's or an inline body's, sorted: what places a unit's statement after them.
    pub before: &'g [(BodyId, u32)],
    /// Each unit of an inline body, by segment and unit, beside the statement of its own it sits
    /// at, sorted.
    pub inline: &'g [((Segment, UnitIndex), GraphSite)],
}

impl<'g> GraphFacts<'g> {
    /// Hand `each` every type these facts name: each name's bound, each term's type, each ask's
    /// and question's, and each path class's entry and resolution.
    pub fn each_type(&self, each: &mut dyn FnMut(DeclaredType<Parametric>)) {
        use crate::scope::Named;
        for name in self.names {
            each(name.upper.into());
        }
        for term in self.terms {
            match *term {
                GraphTerm::Type { ty, .. } => each(ty),
                GraphTerm::Carried { typed, .. } => {
                    each(typed.lower.into());
                    each(typed.upper.into());
                }
                GraphTerm::Name(_) | GraphTerm::Module { .. } => {}
            }
        }
        for ask in self.asks {
            ask.question.each_type(each);
        }
        for question in self.questions {
            match question {
                GraphQuestion::Site { question, .. } => question.each_type(each),
                GraphQuestion::Decides { question, .. } => question.each_type(each),
            }
        }
        for (_, entry, resolution) in self.classes {
            entry.each_named(each);
            resolution.each_named(each);
        }
    }

    pub fn name(&self, name: NameId) -> &'g GraphName {
        &self.names[name.index()]
    }

    pub fn term(&self, term: TermId) -> &'g GraphTerm<'g> {
        &self.terms[term.0 as usize]
    }

    pub fn ask(&self, ask: AskId) -> &'g GraphAsk<'g> {
        &self.asks[ask.0 as usize]
    }

    pub fn question(&self, question: QuestionId) -> &'g GraphQuestion<'g> {
        &self.questions[question.index()]
    }

    /// Where a load walking the text meets the call `edge`, as a key sorting a caller's calls into
    /// that order as [`refusal_order`](Self::refusal_order) sorts refusals: by the end of the span
    /// it is written at, the narrower first, then by the edge's id. A call written nowhere sorts
    /// after every call written.
    pub fn call_order(&self, edge: EdgeId) -> (u32, std::cmp::Reverse<u32>, EdgeId) {
        let written = self.calls.get(edge.index()).and_then(|(_, at)| *at);
        let (start, end) = written.map_or((0, u32::MAX), |at| (at.span.start, at.span.end));
        (end, std::cmp::Reverse(start), edge)
    }

    /// Where a load walking the text meets the refusal at `arm` of the stored question `question`,
    /// as a key sorting the refusals one instance answers into that order. The walk refuses what
    /// lies inside an expression before the expression, so the key is the span of the refusal's
    /// `at` widened to the node its question sits at, where the walk raises it: by its end, the
    /// narrower first, then by the question's id, which orders a site's rules as they are asked.
    pub fn refusal_order(
        &self,
        shapes: &Shapes<'_>,
        question: QuestionId,
        arm: u16,
    ) -> (u32, std::cmp::Reverse<u32>, QuestionId) {
        let stored = (self.question(question).stored()).expect("a refused question is stored");
        let Payload::Refusal(template) = stored.arms[usize::from(arm)] else {
            unreachable!("a refused answer names its refusal's template");
        };
        let at = template.at().span;
        let shape = shapes.shape_at(stored.segment);
        let (start, end) = (shape.body().iter())
            .find_map(|node| crate::scope::source_of(shape, node, stored.site))
            .map_or((at.start, at.end), |site| {
                (at.start.min(site.span.start), at.end.max(site.span.end))
            });
        (end, std::cmp::Reverse(start), question)
    }

    /// The graph question the stored site question of `rule` at `site` of `segment` is laid as:
    /// `None` where the text decides it, or the fold left it out.
    pub fn question_at(
        &self,
        segment: Segment,
        site: crate::scope::Site,
        rule: SiteRule,
    ) -> Option<QuestionId> {
        let key = (segment, site, rule);
        let at = (self.sites)
            .binary_search_by_key(&key, |(held, _)| *held)
            .ok()?;
        Some(self.sites[at].1)
    }

    /// One path of `class` beside its resolution, where the load resolved one.
    pub fn class(&self, class: PathClass) -> Option<(PathEntry<'g>, Resolution<'g>)> {
        let at = (self.classes)
            .binary_search_by_key(&class, |(held, _, _)| *held)
            .ok()?;
        let (_, entry, resolution) = self.classes[at];
        Some((entry, resolution))
    }

    /// Where the call `edge` is written.
    pub fn call(&self, edge: EdgeId) -> Option<SourceRef> {
        self.calls[edge.index()].1
    }

    /// The site of the graph the unit `unit` of `segment`, one of `shapes`'s, sits at.
    pub fn site(&self, shapes: &Shapes<'_>, segment: Segment, unit: UnitIndex) -> GraphSite {
        site_of(shapes, self.bodies, self.before, self.inline, segment, unit)
    }

    /// The statement the part at `site` of `segment` binds at, where it is held in place.
    pub fn held_site(&self, segment: Segment, site: Site) -> Option<GraphSite> {
        let at = (self.held)
            .binary_search_by_key(&(segment, site), |(held, _)| *held)
            .ok()?;
        Some(self.held[at].1)
    }

    /// The first call in text order into `body`.
    pub fn first_call(&self, body: BodyId) -> Option<SourceRef> {
        (self.calls.iter())
            .filter(|(callee, _)| *callee == body)
            .filter_map(|(_, call)| *call)
            .min_by_key(|call| (call.file.0, call.span.start))
    }
}

/// What koan's binding graphs keep: borrowing nothing, so a graph is one type whatever facts it
/// is laid over, and a composition's graph is borrowed as its facts are.
#[derive(Clone, Copy, Debug)]
pub struct GraphKept;

impl Kept for GraphKept {
    type Term = TermId;
    type NameQuestion = AskId;
    /// An ask's answer, `None` where it is undecided: the last of its question's answers.
    type NameAnswer = Option<Answer>;
    type SiteAnswer = SiteAnswer;
}

impl Facts for GraphFacts<'_> {
    type Kept = GraphKept;

    fn mentions(&self, term: TermId) -> Vec<(NameId, Nesting)> {
        match *self.term(term) {
            GraphTerm::Type { mentions, .. } | GraphTerm::Carried { mentions, .. } => {
                mentions.to_vec()
            }
            GraphTerm::Name(name) => vec![(name, Nesting::Bare)],
            GraphTerm::Module { .. } => vec![],
        }
    }

    fn arm(&self, answer: SiteAnswer) -> Option<u32> {
        answer.arm().map(u32::from)
    }

    fn answer_count(&self, ask: AskId) -> u32 {
        self.ask(ask).question.answer_count() + 1
    }

    fn answer_index(&self, ask: AskId, answer: Option<Answer>) -> u32 {
        let question = &self.ask(ask).question;
        answer.map_or(question.answer_count(), |answer| answer.index_in(question))
    }

    fn nth_answer(&self, ask: AskId, index: u32) -> Option<Answer> {
        let question = &self.ask(ask).question;
        (index < question.answer_count()).then(|| question.nth_answer(index))
    }
}

/// The answer function: a site question's own decision, each ask a binding may change asked
/// through the doors, every other answered as the load's door answered it.
pub struct GraphAnswers<'f, 'g> {
    pub facts: &'f GraphFacts<'g>,
}

impl<'g> Answers<GraphFacts<'g>> for GraphAnswers<'_, 'g> {
    fn answer(
        &mut self,
        question: QuestionId,
        doors: &mut Doors<'_, GraphFacts<'g>>,
    ) -> SiteAnswer {
        match *self.facts.question(question) {
            GraphQuestion::Site { question, asks } => {
                let mut through = Through {
                    question,
                    asks,
                    facts: self.facts,
                    doors,
                };
                decide(question, &mut through)
            }
            // A decided name's question has no answer at the load, else its type would be the arm.
            GraphQuestion::Decides { question, ask } => {
                match ask.and_then(|ask| doors.ask(self.facts.ask(ask).names, ask)) {
                    Some(answer) => SiteAnswer::Arm(answer.index_in(&question) as u16),
                    None => SiteAnswer::Undecided,
                }
            }
        }
    }
}

/// The answer function, counting its runs under test.
struct Counted<'f, 'g, 'x> {
    answers: GraphAnswers<'f, 'g>,
    #[cfg(test)]
    runs: &'x std::cell::Cell<u64>,
    #[cfg(not(test))]
    runs: std::marker::PhantomData<&'x ()>,
}

impl<'g> Answers<GraphFacts<'g>> for Counted<'_, 'g, '_> {
    fn answer(
        &mut self,
        question: QuestionId,
        doors: &mut Doors<'_, GraphFacts<'g>>,
    ) -> SiteAnswer {
        #[cfg(test)]
        self.runs.set(self.runs.get() + 1);
        Answers::answer(&mut self.answers, question, doors)
    }
}

/// A question's asks answered through the doors where a binding may change them.
struct Through<'x, 'd, 'g> {
    question: &'g SiteQuestion<'g>,
    asks: &'g [Option<AskId>],
    facts: &'x GraphFacts<'g>,
    doors: &'x mut Doors<'d, GraphFacts<'g>>,
}

impl Asks for Through<'_, '_, '_> {
    fn ask(&mut self, index: u16) -> Option<Answer> {
        let at = usize::from(index);
        match self.asks[at] {
            None => self.question.asks[at].rigid,
            Some(ask) => self.doors.ask(self.facts.ask(ask).names, ask),
        }
    }
}

/// A question's asks answered as the load's doors answered them, noting whether the decision
/// asked one with no answer at every binding.
struct Rigid<'q> {
    question: &'q SiteQuestion<'q>,
    open: bool,
}

impl Asks for Rigid<'_> {
    fn ask(&mut self, index: u16) -> Option<Answer> {
        let rigid = self.question.asks[usize::from(index)].rigid;
        self.open |= rigid.is_none();
        rigid
    }
}

/// Where a base's owner keeps its laid graph: set once, by the load or the composition, and dropped
/// with the owner.
pub type GraphCell = std::cell::OnceCell<bindgraph::Graph<GraphKept>>;

/// One base's [binding graph](../../design/binding-graph.md) — the program's or a composition's —
/// the facts it keeps, the instances its load made of it, frozen in its owner's bump, and where
/// each node's handle sits in a frame of its body. The graph is the one owning heap structure its
/// owner holds, set once in the owner's cell and dropped with the base.
#[derive(Clone, Copy)]
pub struct BindingGraph<'g> {
    pub graph: &'g bindgraph::Graph<GraphKept>,
    pub facts: &'g GraphFacts<'g>,
    pub frozen: &'g bindgraph::Frozen<'g, GraphKept>,
    pub layout: Layout<'g>,
    /// What making the load's instances asked and ran.
    #[cfg(feature = "perf")]
    pub counts: bindgraph::Counts,
}

/// Where each node's handle sits in its body's **handle slab**, the words a frame of the body
/// holds: each node of a body at the next offset, in site order, a word marking whether the frame
/// made its instance, then its handle's width.
#[derive(Clone, Copy, Debug)]
pub struct Layout<'g> {
    /// Each node's offset in its body's slab, by node.
    pub offsets: &'g [u32],
    /// Each body's slab width, by body.
    pub widths: &'g [u32],
}

impl<'g> Layout<'g> {
    /// The layout of `graph`'s nodes, laid through `lay`, counted in `scratch`.
    pub fn of(
        graph: &bindgraph::Graph<GraphKept>,
        lay: impl Lays<'g>,
        scratch: BumpAllocator<'_>,
    ) -> Layout<'g> {
        let mut widths: BumpVec<'_, u32> = BumpVec::new_in(scratch);
        let offsets = lay.fill(graph.nodes().len(), |index| {
            let node = &graph.nodes()[index];
            let body = node.site().body().index();
            if widths.len() <= body {
                widths.resize(body + 1, 0);
            }
            let offset = widths[body];
            widths[body] += 1 + node.handle_width() as u32;
            offset
        });
        let widths = collect(lay, widths.iter().copied());
        Layout { offsets, widths }
    }

    /// Where the word marking whether a frame made `node`'s instance sits in its body's slab.
    pub fn made(&self, node: NodeId) -> usize {
        self.offsets[node.index()] as usize
    }

    /// The words of `node`'s handle among its body's slab, after its mark.
    pub fn range(
        &self,
        graph: &bindgraph::Graph<GraphKept>,
        node: NodeId,
    ) -> std::ops::Range<usize> {
        let start = self.made(node) + 1;
        start..start + graph.node(node).handle_width()
    }

    /// The width of `body`'s slab.
    pub fn width(&self, body: BodyId) -> usize {
        self.widths
            .get(body.index())
            .map_or(0, |width| *width as usize)
    }
}

/// A type a frame's binding closed, by the body it was read in, the type, and each name the
/// binding binds beside its type, or its module's carried type and content digest.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ClosedKey<'k> {
    pub segment: Segment,
    pub ty: DeclaredType<Parametric>,
    pub binding: &'k [(Unknown, KType, Option<ContentDigest>)],
}

impl ClosedKey<'_> {
    /// The word a table of kept closed types homes this key at.
    fn word(&self) -> u64 {
        use std::hash::{BuildHasher, BuildHasherDefault, DefaultHasher};
        BuildHasherDefault::<DefaultHasher>::default().hash_one(self)
    }
}

/// How many views a node of a binding graph keeps that do not last as long as its base's load:
/// views over types the run made, or modules it built.
pub const INSTANCE_SLOTS: usize = 8;

/// The instances run cells make of one base's binding graph — the program's, which the agent
/// keeps, or a composition's, which its depot slot keeps — beside the graph: the crate's `Bound`
/// over the frozen part its load made, its views kept in a bump lent it, and its answers resting
/// there within a budget. A step reaches it through its door and borrows it for one call at a
/// time, since a reader reaches answers through a shared door.
pub struct Instances<'g> {
    pub binding: BindingGraph<'g>,
    /// The load the graph was folded from, and how a name minted over it is spelled.
    pub load: &'g LoadFacts<'g>,
    pub spellings: crate::scope::Spellings<'g>,
    rebind: Rebind<'g>,
    make: Make<'g>,
    bound: std::cell::RefCell<bindgraph::Bound<'g, 'g, 'g, GraphFacts<'g>>>,
    resting: &'g bindgraph::Resting<'g, GraphKept>,
    /// What a frame's binding closed each type to, by [`ClosedKey`]: kept where the type, what the
    /// binding binds and what it closed to each last as long as the load, as a view is.
    closed: crate::memory::DigestTable<'g, (ClosedKey<'g>, crate::scope::Closed)>,
    lend: &'g crate::memory::Bump,
    /// How many times the answer function ran, for a test to read.
    #[cfg(test)]
    runs: std::cell::Cell<u64>,
}

impl crate::scope::SiteAnswers for Instances<'_> {
    fn answered(
        &self,
        segment: Segment,
        site: crate::scope::Site,
        rule: SiteRule,
        handles: crate::scope::Handles<'_>,
    ) -> Option<SiteAnswer> {
        if handles.slab.is_empty() && handles.enclosers.is_empty() {
            return None;
        }
        let laid = self.binding;
        let question = laid.facts.question_at(segment, site, rule)?;
        let owner = laid.graph.question(question).owner();
        let body = laid.graph.node(owner).site().body();
        let owned = |segment: Segment| {
            laid.facts
                .bodies
                .get(segment.index() as usize)
                .copied()
                .flatten()
                == Some(body)
        };
        // A question the frame's own node owns is answered through its slab; one an encloser's
        // node owns, through the words that encloser held when the callable was born.
        if owned(self.load.shapes().owner(segment)) {
            let slab = handles.slab;
            return self.held_answer(owner, |at| slab.get(at).map(std::cell::Cell::get), question);
        }
        let held = (handles.enclosers.iter()).find(|held| owned(held.body))?;
        self.held_answer(owner, |at| held.words.get(at).copied(), question)
    }
}

/// The door a frame's binding is written into a base record's type through, the language's
/// monomorphized in: [`Rebinds`](crate::scope::Rebinds) over these instances.
pub type Rebind<'g> = fn(
    &Instances<'g>,
    &Chain<'_, '_, '_, '_>,
    BumpAllocator<'_>,
    Segment,
    crate::scope::Binding<'_>,
    DeclaredType<Parametric>,
) -> crate::scope::Closed;

/// The door a bind site's instance is made through, the language's monomorphized in: `node`'s
/// instance under what `bound` binds its names to, its handle written into `into`, and the first
/// question it owns that it answers with a refusal, by its arm.
pub type Make<'g> = fn(
    &Instances<'g>,
    NodeId,
    &[(bindgraph::NameId, crate::scope::Bound<'_>)],
    &Chain<'_, '_, '_, '_>,
    BumpAllocator<'_>,
    &mut [u64],
) -> Option<(QuestionId, u16)>;

/// One base's instances as a step borrows them, whichever base they are of: a frame's bind sites
/// make instances through it, and its readers reach their answers and the frame's binding written
/// into a base record's type through it.
pub trait BaseInstances: crate::scope::SiteAnswers + crate::scope::Rebinds {
    /// The base's binding graph.
    fn graph(&self) -> BindingGraph<'_>;

    /// `node`'s instance under what `bound` binds its names to, found filed or made, its handle
    /// written into `into`: and, of the questions it owns that it answers with a refusal, the one
    /// a load walking the text meets first ([`GraphFacts::refusal_order`]), by its arm.
    fn make(
        &self,
        node: NodeId,
        bound: &[(bindgraph::NameId, crate::scope::Bound<'_>)],
        types: &Chain<'_, '_, '_, '_>,
        scratch: BumpAllocator<'_>,
        into: &mut [u64],
    ) -> Option<(QuestionId, u16)>;

    /// How many times the answer function ran for an instance.
    #[cfg(test)]
    fn runs(&self) -> u64;

    /// What the run's instances asked and ran.
    #[cfg(feature = "perf")]
    fn counts(&self) -> bindgraph::Counts;
}

impl BaseInstances for Instances<'_> {
    fn graph(&self) -> BindingGraph<'_> {
        self.binding
    }

    #[cfg(test)]
    fn runs(&self) -> u64 {
        self.runs.get()
    }

    #[cfg(feature = "perf")]
    fn counts(&self) -> bindgraph::Counts {
        self.bound.borrow().counts()
    }

    fn make(
        &self,
        node: NodeId,
        bound: &[(bindgraph::NameId, crate::scope::Bound<'_>)],
        types: &Chain<'_, '_, '_, '_>,
        scratch: BumpAllocator<'_>,
        into: &mut [u64],
    ) -> Option<(QuestionId, u16)> {
        (self.make)(self, node, bound, types, scratch, into)
    }
}

impl crate::scope::Rebinds for Instances<'_> {
    fn rebound(
        &self,
        types: &Chain<'_, '_, '_, '_>,
        scratch: BumpAllocator<'_>,
        segment: Segment,
        binding: crate::scope::Binding<'_>,
        ty: DeclaredType<Parametric>,
    ) -> crate::scope::Closed {
        (self.rebind)(self, types, scratch, segment, binding, ty)
    }

    #[cfg(test)]
    fn foreign(&self, types: &Chain<'_, '_, '_, '_>, ty: DeclaredType<Parametric>) -> bool {
        let scratch = crate::memory::Bump::new();
        (crate::scope::free_variables(types, &scratch, ty).iter()).any(|variable| {
            matches!(variable, lattice::types::Variable::Derived { term, .. }
                if self.load.derived_name(*term).is_none())
        })
    }
}

impl<'g> Instances<'g> {
    /// The instances of `binding`, folded from `load`, kept in `lend`, answers resting there
    /// within `budget` bytes, each node keeping [`INSTANCE_SLOTS`] views that do not last. A
    /// frame's binding is written into a type through `rebind`, and an instance made through
    /// `make`.
    pub fn new(
        binding: BindingGraph<'g>,
        (load, spellings): (&'g LoadFacts<'g>, crate::scope::Spellings<'g>),
        (rebind, make): (Rebind<'g>, Make<'g>),
        lend: &'g crate::memory::Bump,
        budget: usize,
    ) -> Self {
        let bound = bindgraph::Bound::new(
            binding.graph,
            binding.facts,
            binding.frozen,
            lend,
            Some(INSTANCE_SLOTS),
        );
        Instances {
            binding,
            load,
            spellings,
            rebind,
            make,
            bound: std::cell::RefCell::new(bound),
            resting: bindgraph::Resting::new(lend, Some(budget)),
            closed: crate::memory::DigestTable::new(lend),
            lend,
            #[cfg(test)]
            runs: std::cell::Cell::new(0),
        }
    }

    /// The handle of `node`'s instance under `binding`, asked through `domain`, written into
    /// `into`: found filed, or made. Of the questions `node` owns that the instance answers with a
    /// refusal, the one a load walking the text meets first ([`GraphFacts::refusal_order`]), by
    /// its arm: the refusal its bind site faults with.
    pub fn instance<D: bindgraph::Domain<Facts = GraphFacts<'g>>>(
        &self,
        node: NodeId,
        binding: &bindgraph::Binding<D>,
        domain: &D,
        into: &mut [u64],
    ) -> Option<(QuestionId, u16)> {
        let mut answers = Counted {
            answers: GraphAnswers {
                facts: self.binding.facts,
            },
            #[cfg(test)]
            runs: &self.runs,
            #[cfg(not(test))]
            runs: std::marker::PhantomData,
        };
        let bound = &mut *self.bound.borrow_mut();
        let handle = bound.instance(self.resting, node, binding, domain, &mut answers, into);
        let owns = self.binding.graph.node(node).owns();
        let (facts, shapes) = (self.binding.facts, self.load.shapes());
        (owns.iter())
            .filter_map(|&question| {
                match bindgraph::Bound::answer(bound, self.resting, handle, question, &mut answers)
                {
                    SiteAnswer::Refused(arm) => Some((question, arm)),
                    _ => None,
                }
            })
            .min_by_key(|(question, arm)| facts.refusal_order(shapes, *question, *arm))
    }

    /// What a frame's binding closed a type to, under `key`, where a frame kept it.
    pub fn kept_closed(&self, key: ClosedKey<'_>) -> Option<crate::scope::Closed> {
        let found = self.closed.find(key.word(), |(held, _)| *held == key);
        found.map(|(_, closed)| *closed)
    }

    /// Keeps what a frame's binding closed a type to, under `key`; every part of both lasts. The
    /// entries rest in the lent bump outside the instances' budget: a caller keeps one only where
    /// the type, the binding and the closed type are all the load's, so there are finitely many,
    /// as there are views.
    pub fn keep_closed(&self, key: ClosedKey<'_>, closed: crate::scope::Closed) {
        let binding =
            crate::memory::LaysRuns::lay(self.lend, key.binding.len(), |at| key.binding[at]);
        let kept = ClosedKey { binding, ..key };
        let entry = &crate::memory::LaysRuns::lay(self.lend, 1, |_| (kept, closed))[0];
        self.closed.insert_at(self.lend, key.word(), entry);
    }

    /// The answer to `question` of the instance of `node` whose handle is `words`.
    pub fn answer_of(&self, node: NodeId, words: &[u64], question: QuestionId) -> SiteAnswer {
        let mut answers = GraphAnswers {
            facts: self.binding.facts,
        };
        let handle = bindgraph::Handle::new(node, words);
        let bound = &mut *self.bound.borrow_mut();
        bindgraph::Bound::answer(bound, self.resting, handle, question, &mut answers)
    }

    /// The answer `node`'s instance, held in the slab whose word at each offset is `word`, gives
    /// `question`: `None` where the frame holding the slab made no instance of it.
    fn held_answer(
        &self,
        node: NodeId,
        word: impl Fn(usize) -> Option<u64>,
        question: QuestionId,
    ) -> Option<SiteAnswer> {
        let laid = self.binding;
        if word(laid.layout.made(node))? == 0 {
            return None;
        }
        let words: smallvec::SmallVec<[u64; 8]> = (laid.layout.range(laid.graph, node))
            .map(&word)
            .collect::<Option<_>>()?;
        Some(self.answer_of(node, &words, question))
    }

    /// How many edges the instances have walked.
    pub fn walked(&self) -> u64 {
        self.bound.borrow().walked()
    }
}

/// The text of one base's graph beside the facts it keeps.
pub struct Folded<'g> {
    pub text: Text<GraphKept>,
    pub facts: GraphFacts<'g>,
}

/// The text of the binding graph of the base `load` holds, its tables laid through `lay`.
pub fn folded<'g>(
    load: &LoadFacts<'g>,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    lay: impl Lays<'g>,
) -> Folded<'g> {
    let shapes = load.shapes();
    let mut fold = Fold {
        shapes,
        load,
        types,
        scratch,
        lay,
        text: Text::new(),
        bodies: BumpVec::new_in(scratch),
        held: BumpVec::new_in(scratch),
        before: BumpVec::new_in(scratch),
        inline: BumpVec::new_in(scratch),
        enclosers: BumpVec::new_in(scratch),
        names: BumpVec::new_in(scratch),
        named: bump_table(scratch),
        answered: bump_table(scratch),
        classes: bump_table(scratch),
        terms: BumpVec::new_in(scratch),
        asks: BumpVec::new_in(scratch),
        questions: BumpVec::new_in(scratch),
        sites: BumpVec::new_in(scratch),
        calls: BumpVec::new_in(scratch),
        returned: bump_table(scratch),
        left: 0,
    };
    fold.lay_bodies();
    for (_, (facts, _)) in load.own_units() {
        for (_, (entry, resolution)) in facts.paths.iter() {
            if let Resolved::Class(class) = resolution.resolved {
                fold.classes.entry(class).or_insert((*entry, *resolution));
            }
        }
    }
    let stored = fold.stored();
    // Every name a question reads first, so a call's mapping finds a callee's name with its bound.
    for (_, question) in stored.iter() {
        for posed in question.asks {
            posed.question.each_type(&mut |ty| {
                let (mut read, mut through) = (BumpVec::new_in(scratch), BumpVec::new_in(scratch));
                fold.reads(question.segment, ty, &mut read, Some(&mut through));
            });
        }
    }
    let mut selections = BumpVec::new_in(scratch);
    for (site, question) in stored.iter() {
        if question.rule == SiteRule::Selection {
            selections.push((question.segment, question.site));
        }
        fold.question(*site, question);
    }
    let typed = (load.own_units()).flat_map(|(unit, (facts, _))| {
        (facts.typed.entries().iter()).map(move |(segment, typed)| (unit, *segment, typed))
    });
    for (unit, segment, typed) in typed {
        for (site, traced) in typed.traced.entries() {
            if selections.contains(&(segment, *site)) {
                continue;
            }
            let Some(callee) = fold.body(traced.callee) else {
                continue;
            };
            let (mapping, _) =
                fold.slots_mapped((segment, *site), traced.callee, traced.passes, None);
            let at = fold.call_site((segment, *site), unit);
            let edge = fold.text.passes_at(at, callee, mapping.to_vec());
            fold.calls.push(LaidCall {
                callee,
                written: Some(traced.call),
                held: Some(HeldCall {
                    edge,
                    at: (segment, *site),
                    on: None,
                    into: traced.callee,
                }),
            });
        }
    }
    fold.lay_holders();
    for shape in shapes.bodies() {
        let segment = shape.segment();
        let Some(body) = fold.body(segment) else {
            continue;
        };
        let offered = (shapes.calls_into(segment).iter()).any(|call| call.offered);
        if shape.kind() == ShapeKind::Code || shapes.escapes(segment) || offered {
            fold.text.open_caller(body);
        }
    }
    let facts = GraphFacts {
        names: collect(lay, fold.names.iter().copied()),
        terms: collect(lay, fold.terms.iter().copied()),
        asks: collect(lay, fold.asks.iter().copied()),
        questions: collect(lay, fold.questions.iter().copied()),
        sites: {
            let mut sites = BumpVec::with_capacity_in(fold.sites.len(), scratch);
            sites.extend(fold.sites.iter().copied());
            sites.sort_by_key(|(key, _)| *key);
            collect(lay, sites.iter().copied())
        },
        calls: collect(
            lay,
            (fold.calls.iter()).map(|call| (call.callee, call.written)),
        ),
        classes: {
            let mut classes = BumpVec::with_capacity_in(fold.classes.len(), scratch);
            classes.extend(
                (fold.classes.iter())
                    .map(|(class, (entry, resolution))| (*class, *entry, *resolution)),
            );
            classes.sort_by_key(|(class, _, _)| *class);
            collect(lay, classes.iter().copied())
        },
        left: fold.left,
        bodies: collect(lay, fold.bodies.iter().copied()),
        held: collect(lay, fold.held.iter().copied()),
        before: collect(lay, fold.before.iter().copied()),
        inline: collect(lay, fold.inline.iter().copied()),
    };
    Folded {
        text: fold.text,
        facts,
    }
}

/// What a decided derived name is to the graph: an answered name, constant at every binding the
/// chain reading it sees, or read through its question and arms.
#[derive(Clone, Copy, Debug)]
enum Answered {
    Name(NameId),
    Constant,
    Through,
}

/// A call's callee, what it maps the callee's names to, and the names the mapping reads.
type Mapped<'x> = (BodyId, BumpVec<'x, (NameId, TermId)>, BumpVec<'x, NameId>);

/// The fold's state: the text so far, and the tables its ids index.
struct Fold<'f, 'g, 'x, 'tier, 'pair, 'reg, 'door, L> {
    shapes: &'g Shapes<'g>,
    load: &'f LoadFacts<'g>,
    types: &'f Chain<'tier, 'pair, 'reg, 'door>,
    scratch: BumpAllocator<'x>,
    /// Where the facts the graph keeps are laid.
    lay: L,
    text: Text<GraphKept>,
    /// Each owning body's id, by segment.
    bodies: BumpVec<'x, Option<BodyId>>,
    /// Each part held in place, by segment and site, beside its statement; the owning body and
    /// unit run index of each statement laid before a unit's own; and each inline body's unit
    /// beside its statement: all sorted ([`GraphFacts::held`], [`GraphFacts::before`],
    /// [`GraphFacts::inline`]).
    held: BumpVec<'x, ((Segment, Site), GraphSite)>,
    before: BumpVec<'x, (BodyId, u32)>,
    inline: BumpVec<'x, ((Segment, UnitIndex), GraphSite)>,
    /// Each laid body's encloser, by its id.
    enclosers: BumpVec<'x, Option<BodyId>>,
    names: BumpVec<'x, GraphName>,
    /// Each name by its unknown and which name there it is.
    named: BumpBackedMap<'x, (Unknown, NameKind), NameId>,
    /// What each decided derived name is to the graph, by the segment reading it — whose levels
    /// its types are written in — and its id.
    answered: BumpBackedMap<'x, (Segment, DerivedId), Answered>,
    /// One path of each path class beside its resolution: what the class reads.
    classes: BumpBackedMap<'x, PathClass, (PathEntry<'g>, Resolution<'g>)>,
    terms: BumpVec<'x, GraphTerm<'g>>,
    asks: BumpVec<'x, GraphAsk<'g>>,
    questions: BumpVec<'x, GraphQuestion<'g>>,
    sites: BumpVec<'x, (SiteKey, QuestionId)>,
    calls: BumpVec<'x, LaidCall>,
    /// What each callable's result carries to its callers, by its body, once asked, or that it is
    /// being asked.
    returned: BumpBackedMap<'x, Segment, held::Asked>,
    left: usize,
}

/// A call laid, by its edge's id: its callee and where it is written, and where a call of the text
/// sits, what holds its value — the site it is written at, the arm it hangs on, and the callee's
/// segment.
#[derive(Clone, Copy)]
struct LaidCall {
    callee: BodyId,
    written: Option<SourceRef>,
    held: Option<HeldCall>,
}

/// Where a call of the text sits: its edge, its site, the arm of its selection question it hangs on, `None`
/// where the text selects it, and its callee's segment.
#[derive(Clone, Copy)]
struct HeldCall {
    edge: EdgeId,
    at: (Segment, Site),
    on: Option<(QuestionId, u32)>,
    into: Segment,
}

/// What a callable's result carries to its callers: a closed term, or an answered name of the
/// callable typed through the calls its last statement makes.
#[derive(Clone, Copy, Debug)]
enum Returned {
    Fixed(TermId),
    Name(NameId),
}

impl<'g, 'x, L: Lays<'g>> Fold<'_, 'g, 'x, '_, '_, '_, '_, L> {
    /// Lays each owning body, an encloser before what it encloses, with its statements: one per
    /// unit, then one for its closing.
    fn lay_bodies(&mut self) {
        let shapes = self.shapes;
        self.bodies.extend((0..shapes.bodies().len()).map(|_| None));
        // A body's segment exceeds every body's it encloses, so the root comes first from the end.
        for shape in shapes.bodies().iter().rev() {
            let segment = shape.segment();
            let (body, encloser) = match (shape.kind(), shapes.parent(segment)) {
                (ShapeKind::Block | ShapeKind::Module, _) => continue,
                // Composed code is entered from outside the text, its entry binding its captures
                // and parameters: a body open to callers inside a root that binds nothing.
                (ShapeKind::Code, None) => {
                    let root = self.text.root();
                    (self.text.body(root), Some(root))
                }
                (_, None) => (self.text.root(), None),
                (_, Some((parent, _))) => {
                    let encloser = self.body(shapes.owner(parent));
                    let encloser = encloser.expect("an encloser is laid first");
                    (self.text.body(encloser), Some(encloser))
                }
            };
            let at = body.index();
            if self.enclosers.len() <= at {
                self.enclosers.resize(at + 1, None);
            }
            self.enclosers[at] = encloser;
            self.bodies[segment.index() as usize] = Some(body);
            // A quote's code runs where an `EVAL` runs it: what it asks is asked at its entry.
            if shape.kind() == ShapeKind::Code {
                self.text.staged(body);
            }
            for run in 0..shape.units().len() as u32 {
                self.lay_before(body, run, segment, UnitIndex::of_run(run as usize));
                self.text.statement(body);
            }
            self.text.statement(body);
        }
        self.held.sort_unstable_by_key(|(part, _)| *part);
        self.before.sort_unstable();
        self.inline.sort_unstable_by_key(|(unit, _)| *unit);
    }

    /// Lays, in `body`, each statement the run binds before the unit `unit` of `segment`, which
    /// the owning body's unit `run` runs: each part the unit holds in place, inner first then left
    /// to right, after the inline bodies evaluating it runs; then the inline bodies the unit runs
    /// itself.
    fn lay_before(&mut self, body: BodyId, run: u32, segment: Segment, unit: UnitIndex) {
        let shape = self.shapes.shape_at(segment);
        // Each inline body the unit runs, by its site, beside whether it is laid yet.
        let mut inline = BumpVec::new_in(self.scratch);
        for statement in 0..shape.body().len() {
            if shape.unit_of_statement(statement) != unit {
                continue;
            }
            for (site, nested) in shape.nested_in(statement) {
                if let ShapeKind::Block | ShapeKind::Module = nested.kind() {
                    inline.push((*site, nested.segment(), false));
                }
            }
        }
        let mut parts = BumpVec::new_in(self.scratch);
        parts.extend(
            (self.load.held_sites(shape).iter().copied())
                .filter(|site| shape.unit_of_site(*site) == unit),
        );
        // Sites number an outer part before the parts inside it: each part closes once the next
        // lies outside it.
        let mut open = BumpVec::<Site>::new_in(self.scratch);
        for at in 0..=parts.len() {
            while let Some(&top) = open.last() {
                if parts.get(at).is_some_and(|site| shape.within(top, *site)) {
                    break;
                }
                for (site, nested, laid) in inline.iter_mut() {
                    if !*laid && shape.within(top, *site) {
                        *laid = true;
                        self.lay_inline(body, run, *nested);
                    }
                }
                let statement = self.text.statement(body);
                self.held.push(((segment, top), statement));
                self.before.push((body, run));
                open.pop();
            }
            if let Some(site) = parts.get(at) {
                open.push(*site);
            }
        }
        for (_, nested, laid) in inline.iter() {
            if !*laid {
                self.lay_inline(body, run, *nested);
            }
        }
    }

    /// Lays the inline body `segment`, a block or a `MODULE` body the owning body's unit `run`
    /// runs, in `body`, as an owning body is laid: a statement for its entry, then each unit's
    /// after what the run binds before it.
    fn lay_inline(&mut self, body: BodyId, run: u32, segment: Segment) {
        let statement = |fold: &mut Self, unit: UnitIndex| {
            let at = fold.text.statement(body);
            fold.before.push((body, run));
            fold.inline.push(((segment, unit), at));
        };
        statement(self, UnitIndex::ENTRY);
        for unit in 0..self.shapes.shape_at(segment).units().len() {
            let unit = UnitIndex::of_run(unit);
            self.lay_before(body, run, segment, unit);
            statement(self, unit);
        }
    }

    fn body(&self, segment: Segment) -> Option<BodyId> {
        self.bodies[segment.index() as usize]
    }

    /// Whether a question at `asked` may read the name `name`: one bound no later in its body, or
    /// in a body enclosing it, as the crate reads a chain.
    fn visible(&self, name: NameId, asked: GraphSite) -> bool {
        let NameAddress::Site(unknown) = self.names[name.index()].address else {
            return true;
        };
        let bound = self.bound_at(unknown);
        let rank = |site: GraphSite| match site {
            GraphSite::Entry(_) => 0,
            GraphSite::Statement(_, index) => index + 1,
        };
        if bound.body() == asked.body() {
            return rank(bound) <= rank(asked);
        }
        let mut body = self.enclosers[asked.body().index()];
        while let Some(encloser) = body {
            if encloser == bound.body() {
                return true;
            }
            body = self.enclosers[encloser.index()];
        }
        false
    }

    /// Every stored site question of the base, beside the site of the graph it sits at: a unit's
    /// at its statement, a closing's at its body's last.
    fn stored(&self) -> BumpVec<'x, (GraphSite, &'g SiteQuestion<'g>)> {
        let mut stored = BumpVec::new_in(self.scratch);
        for (unit, (facts, _)) in self.load.own_units() {
            let site = self.site(unit.segment, unit.unit);
            stored.extend(
                facts
                    .questions
                    .entries()
                    .iter()
                    .map(|question| (site, question)),
            );
        }
        for (segment, (closing, _)) in self.load.own_closings() {
            let site = self.closing_site(segment);
            stored.extend(
                closing
                    .questions
                    .entries()
                    .iter()
                    .map(|question| (site, question)),
            );
        }
        stored
    }

    /// The site of the graph the unit `unit` of `segment` sits at.
    fn site(&self, segment: Segment, unit: UnitIndex) -> GraphSite {
        site_of(
            self.shapes,
            &self.bodies,
            &self.before,
            &self.inline,
            segment,
            unit,
        )
    }

    /// The site of the graph `unknown` binds at: a part held in place at its own statement, any
    /// other at its unit's.
    fn bound_at(&self, unknown: Unknown) -> GraphSite {
        if let Address::Held(site) = unknown.at
            && let Ok(at) =
                (self.held).binary_search_by_key(&(unknown.segment, site), |(held, _)| *held)
        {
            return self.held[at].1;
        }
        let bound = self.shapes.bind_site(unknown);
        self.site(bound.segment, bound.unit)
    }

    /// The site of the graph a call at `site` of `segment`, run by `unit`, is laid at: the
    /// statement of the innermost part held in place that holds it, else its unit's.
    fn call_site(&self, (segment, site): (Segment, Site), unit: crate::scope::UnitId) -> GraphSite {
        let shape = self.shapes.shape_at(segment);
        let held = (self.held.iter())
            .filter(|((at, part), _)| *at == segment && shape.within(*part, site))
            .max_by_key(|((_, part), _)| *part);
        match held {
            Some((_, statement)) => *statement,
            None => self.site(unit.segment, unit.unit),
        }
    }

    /// The site of `segment`'s closing record: its body's last statement.
    fn closing_site(&self, segment: Segment) -> GraphSite {
        match self.body(segment) {
            Some(body) => {
                let units = self.shapes.shape_at(segment).units().len();
                let from = self.before.partition_point(|(each, _)| *each < body);
                let holders = self.before[from..]
                    .iter()
                    .take_while(|(each, _)| *each == body);
                GraphSite::Statement(body, (units + holders.count()) as u32)
            }
            None => {
                let running =
                    (self.shapes.birth_unit(segment)).expect("an inline body has an encloser");
                self.site(running.segment, running.unit)
            }
        }
    }

    /// The name `kind` of those `unknown` binds, laid at the site it binds at on first sight. A
    /// capture crossing into a quote's code is the name its source reads where the code is born. A
    /// name first laid under no known bound takes the one a reader knows.
    fn site_name(&mut self, mut unknown: Unknown, kind: NameKind, upper: KType) -> NameId {
        while let Some(source) = self.shapes.crossed(unknown) {
            unknown = source;
        }
        if let Some(&name) = self.named.get(&(unknown, kind)) {
            let held = &mut self.names[name.index()];
            if held.upper == KType::ANY {
                held.upper = upper;
            }
            return name;
        }
        let name = self.text.name(self.bound_at(unknown));
        self.names.push(GraphName {
            address: NameAddress::Site(unknown),
            kind,
            upper,
        });
        self.named.insert((unknown, kind), name);
        name
    }

    /// The name a call into `callee` maps at its slot `slot`: a type name bound at its entry.
    fn entry_name(&mut self, callee: Segment, slot: crate::scope::Slot) -> Option<NameId> {
        self.shapes.level(callee, Address::Slot(slot))?;
        let unknown = Unknown {
            segment: callee,
            at: Address::Slot(slot),
        };
        let entry = self.shapes.bind_site(unknown).unit == UnitIndex::ENTRY;
        entry.then(|| self.site_name(unknown, NameKind::Type, KType::ANY))
    }

    /// The carried name a call into `callee` maps at its slot `slot`: a parameter declared `Any`,
    /// bound at its entry.
    fn entry_carried(&mut self, callee: Segment, slot: Slot) -> Option<NameId> {
        let unknown = Unknown {
            segment: callee,
            at: Address::Slot(slot),
        };
        (self.shapes.bind_site(unknown).unit == UnitIndex::ENTRY)
            .then(|| self.site_name(unknown, NameKind::Carried, KType::ANY))
    }

    /// The carried and content names a call into `callee` maps at its slot `slot`: a module
    /// parameter's, bound at its entry, whose closed declared type is a signature.
    fn entry_module(&mut self, callee: Segment, slot: Slot) -> Option<(NameId, NameId)> {
        let unknown = Unknown {
            segment: callee,
            at: Address::Slot(slot),
        };
        if self.shapes.bind_site(unknown).unit != UnitIndex::ENTRY {
            return None;
        }
        // The declared type its frame retypes the argument to: its closed type's parameter's.
        let shape = self.shapes.shape_at(callee);
        let crate::scope::Static::Closed(callable) = self.load.callable_type(self.types, shape)
        else {
            return None;
        };
        let TypeNode::KFunction { params, .. } = callable.ktype.view(self.types).body else {
            return None;
        };
        let declared = params.get(shape.slot_name(slot).symbol())?;
        // A parameter whose declared type holds a scheme variable maps no module names.
        let declared = self.types.concrete(declared)?;
        if declared.kind_of(self.types) != KKind::Signature {
            return None;
        }
        Some((
            self.site_name(unknown, NameKind::Carried, KType::ANY),
            self.site_name(unknown, NameKind::Content, KType::ANY),
        ))
    }

    /// Each name `ty`, read in `segment`, is written over, gathered into `into` once each: a type
    /// name by its level, a module binder's carried name, and through any other derived name the
    /// names its term reads. A path class's variable stands for no name: where `through` gathers
    /// them, the names its recipe reads ([`class_reads`](Self::class_reads)). `false` where it reads
    /// a name no binding writes in.
    fn reads(
        &mut self,
        segment: Segment,
        ty: DeclaredType<Parametric>,
        into: &mut BumpVec<'_, (NameId, NameVariable)>,
        mut through: Option<&mut BumpVec<'_, NameId>>,
    ) -> bool {
        let push = |into: &mut BumpVec<'_, (NameId, NameVariable)>, read| {
            if !into.contains(&read) {
                into.push(read);
            }
        };
        for variable in free_variables(self.types, self.scratch, ty).iter() {
            match *variable {
                Variable::Lexical { level, bound, .. } => {
                    let Some(unknown) = self.shapes.unknown_of_level(segment, level) else {
                        return false;
                    };
                    let name = self.site_name(unknown, NameKind::Type, bound);
                    push(into, (name, NameVariable::Level(level)));
                }
                Variable::Derived { term, bound, .. } => {
                    let Some(name) = self.load.derived_name(term) else {
                        return false;
                    };
                    match name.term {
                        DerivedTerm::Carried { binder, .. } => {
                            let name = self.site_name(binder, NameKind::Carried, bound);
                            push(into, (name, NameVariable::Derived(term)));
                            continue;
                        }
                        DerivedTerm::Rigid { .. } | DerivedTerm::Open { .. } => return false,
                        DerivedTerm::Decided {
                            decision: Decider::Answer(question),
                            arms,
                        } => match self.answered(segment, (term, bound), question, arms) {
                            Some(Answered::Name(name)) => {
                                push(into, (name, NameVariable::Derived(term)));
                                continue;
                            }
                            Some(Answered::Constant) => continue,
                            Some(Answered::Through) => {}
                            None => return false,
                        },
                        _ => {}
                    }
                    let mut read = true;
                    name.term.each_type(&mut |operand| {
                        read = read && self.reads(segment, operand, into, through.as_deref_mut());
                    });
                    if !read {
                        return false;
                    }
                }
                Variable::Path { class, .. } => {
                    let Some(through) = through.as_deref_mut() else {
                        return false;
                    };
                    if !self.class_reads(segment, class, into, through) {
                        return false;
                    }
                }
                Variable::Quantified { .. }
                | Variable::Parameter { .. }
                | Variable::Projection { .. } => {}
            }
        }
        true
    }

    /// Each name the class `class`, read in `segment`, reads, gathered into `into` as [`reads`]
    /// gathers them and into `through` where no variable spells it: the names its application is
    /// written over, and those its root's content tree reads ([`tree_reads`](Self::tree_reads)) —
    /// a root binder the run binds is its tree's bare leaf. `false` where it reads a name no binding writes in,
    /// or the load holds no path of the class.
    ///
    /// [`reads`]: Self::reads
    fn class_reads(
        &mut self,
        segment: Segment,
        class: PathClass,
        into: &mut BumpVec<'_, (NameId, NameVariable)>,
        through: &mut BumpVec<'_, NameId>,
    ) -> bool {
        let Some(&(entry, resolution)) = self.classes.get(&class) else {
            return false;
        };
        if let Some(application) = entry.through
            && !self.reads(segment, application.into(), into, Some(through))
        {
            return false;
        }
        self.tree_reads(segment, resolution.tree, false, into, through)
    }

    /// Each name the content tree `tree`, read in `segment`, reads, gathered as
    /// [`class_reads`](Self::class_reads) gathers them: a bound binder's carried name, and its
    /// content name too where the tree reads the binder's identity — anywhere but a bare leaf
    /// (`within`); each name the types it holds are written over; and the names a call leaf's
    /// selection and arguments read where every candidate [unfolds](Self::unfolds), so a binding
    /// that selects one reads the result off its summary, else its holder's alone: the run binds
    /// the holder whole at its seam, where the question is answered.
    fn tree_reads(
        &mut self,
        segment: Segment,
        tree: Tree<'g>,
        within: bool,
        into: &mut BumpVec<'_, (NameId, NameVariable)>,
        through: &mut BumpVec<'_, NameId>,
    ) -> bool {
        let ty = |fold: &mut Self, ty: Parametric, into: &mut BumpVec<'_, _>, through: &mut _| {
            fold.reads(segment, ty.into(), into, Some(through))
        };
        match tree {
            Tree::Digest(..) | Tree::Leaf(Leaf::Unnamed(_)) => true,
            Tree::Body { captures, .. } => (captures.iter())
                .all(|(_, capture)| self.tree_reads(segment, *capture, true, into, through)),
            Tree::Knot { members, .. } => members.iter().all(|member| {
                member
                    .instance
                    .iter()
                    .all(|point| ty(self, *point, into, through))
                    && member.captures.iter().all(|(_, capture)| match capture {
                        NodeCapture::Fellow(_) => true,
                        NodeCapture::Tree(tree) => {
                            self.tree_reads(segment, *tree, true, into, through)
                        }
                    })
            }),
            Tree::View {
                application,
                source,
                underlying,
                ..
            } => {
                ty(self, application, into, through)
                    && self.tree_reads(segment, *source, true, into, through)
                    && self.tree_reads(segment, *underlying, true, into, through)
            }
            Tree::Retyped {
                carried, source, ..
            } => {
                ty(self, carried, into, through)
                    && self.tree_reads(segment, *source, true, into, through)
            }
            Tree::TypeValue { named, .. } => ty(self, named, into, through),
            Tree::Leaf(Leaf::Bound {
                binder,
                within: None,
                ..
            }) => {
                let carried = self.site_name(binder.unknown(), NameKind::Carried, KType::ANY);
                push_name(through, carried);
                if within {
                    let content = self.site_name(binder.unknown(), NameKind::Content, KType::ANY);
                    push_name(through, content);
                }
                true
            }
            // A binder of another run is what that run's captures or call make it.
            Tree::Leaf(Leaf::Bound {
                within: Some(run), ..
            }) => match run {
                Within::Body(captures) => (captures.iter())
                    .all(|(_, capture)| self.tree_reads(segment, *capture, true, into, through)),
                Within::Call(call) => {
                    (call.arguments.iter()).all(|(_, argument)| {
                        self.tree_reads(segment, *argument, true, into, through)
                    }) && (call.solution.iter()).all(|(_, point)| ty(self, *point, into, through))
                }
            },
            Tree::Leaf(Leaf::Call { call, holder, .. }) => {
                if let Some(holder) = holder.filter(|_| !self.unfolds(call)) {
                    for kind in [NameKind::Carried, NameKind::Content] {
                        let name = self.site_name(holder.unknown(), kind, KType::ANY);
                        push_name(through, name);
                    }
                    return true;
                }
                let mut read = true;
                for ask in call.selection.asks {
                    ask.each_type(&mut |asked| {
                        read = read && self.reads(call.at, asked, into, Some(through));
                    });
                }
                read && (call.arguments.iter())
                    .all(|argument| self.tree_reads(segment, *argument, true, into, through))
            }
        }
    }

    /// Whether every candidate of `call` some binding may run leaves the run no module to bind, so
    /// that a binding deciding its selection reads the result off the callee's summary: a builtin,
    /// which returns no module, or a callable body outside a recursion, with no `FOR ALL` group
    /// and a summary holding no call only an answer selects, which may be the callee's own.
    fn unfolds(&self, call: &Called<'_>) -> bool {
        let selection = call.selection;
        (0..selection.candidates.len() as u16).all(|at| {
            let candidate = selection.candidates[usize::from(at)];
            let callee = call.callees[usize::from(candidate.position)];
            candidate.builtin
                || selection.rigid_runs(self.types, self.scratch, at) == Some(Run::Never)
                || !callee.quantified
                    && callee.body.is_some_and(|body| {
                        self.load.closing(body).is_some_and(|(closing, _)| {
                            !closing.recursive
                                && matches!(closing.summary, Some(Some(summary)) if !holds_call(summary))
                        })
                    })
        })
    }

    /// The term `ty`, read in `segment`, beside the names it reads; `None` where it reads a name no
    /// binding writes in.
    fn type_term(
        &mut self,
        segment: Segment,
        ty: DeclaredType<Parametric>,
    ) -> Option<(TermId, BumpVec<'x, NameId>)> {
        let (mentions, variables, names) = self.written_over(segment, &[ty])?;
        let term = self.term(GraphTerm::Type {
            ty,
            mentions,
            variables,
        });
        Some((term, names))
    }

    /// The carried term over `typed`'s ends, read in `segment`, beside the names they read, as
    /// [`type_term`](Self::type_term) reads them.
    fn carried_term(
        &mut self,
        segment: Segment,
        typed: Interval,
    ) -> Option<(TermId, BumpVec<'x, NameId>)> {
        let ends = [typed.lower.into(), typed.upper.into()];
        let (mentions, variables, names) = self.written_over(segment, &ends)?;
        let term = self.term(GraphTerm::Carried {
            typed,
            mentions,
            variables,
        });
        Some((term, names))
    }

    /// The names `tys`, read in `segment`, mention and how, the variable each is in them, and the
    /// names they read; `None` where one reads a name no binding writes in.
    #[allow(clippy::type_complexity)]
    fn written_over(
        &mut self,
        segment: Segment,
        tys: &[DeclaredType<Parametric>],
    ) -> Option<(
        &'g [(NameId, Nesting)],
        &'g [(NameId, NameVariable)],
        BumpVec<'x, NameId>,
    )> {
        let mut read = BumpVec::new_in(self.scratch);
        for ty in tys {
            if !self.reads(segment, *ty, &mut read, None) {
                return None;
            }
        }
        let variables: BumpVec<'_, NameVariable> = {
            let mut variables = BumpVec::with_capacity_in(read.len(), self.scratch);
            variables.extend(read.iter().map(|(_, variable)| *variable));
            variables
        };
        let load = self.load;
        let terms = |id| load.derived_name(id);
        // A name two variables spell, or two types, is mentioned as the deeper.
        let mut by_name: BumpVec<'_, (NameId, Nesting)> = BumpVec::new_in(self.scratch);
        for ty in tys {
            let found = mentions(self.types, self.scratch, &terms, *ty, &variables);
            for (at, how) in found.iter() {
                let name = read[*at].0;
                match by_name.iter_mut().find(|(held, _)| *held == name) {
                    Some((_, held)) if depth_rank(*how) > depth_rank(*held) => *held = *how,
                    Some(_) => {}
                    None => by_name.push((name, *how)),
                }
            }
        }
        let mut names = BumpVec::with_capacity_in(read.len(), self.scratch);
        for (name, _) in read.iter() {
            if !names.contains(name) {
                names.push(*name);
            }
        }
        Some((
            collect(self.lay, by_name.iter().copied()),
            collect(self.lay, read.iter().copied()),
            names,
        ))
    }

    /// The term a call passes where the walk fixed what it passes: load content, or a name of the
    /// caller's `caller` passed on bare. `None` for a module's content, which no term writes.
    fn fixed_term(
        &mut self,
        caller: Segment,
        fixed: Fixed,
    ) -> Option<(TermId, BumpVec<'x, NameId>)> {
        match fixed {
            Fixed::Content(Content::Type(ty)) => {
                self.type_term(caller, DeclaredType::Type(Parametric::from(ty)))
            }
            Fixed::Content(_) => None,
            Fixed::Passed(slot) => {
                self.shapes.level(caller, Address::Slot(slot))?;
                let unknown = Unknown {
                    segment: caller,
                    at: Address::Slot(slot),
                };
                let name = self.site_name(unknown, NameKind::Type, KType::ANY);
                let mut names = BumpVec::new_in(self.scratch);
                names.push(name);
                Some((self.term(GraphTerm::Name(name)), names))
            }
        }
    }

    /// The terms a call maps a module parameter's carried and content names to, where the walk
    /// fixed what it passes, beside the name each reads: a module the text fixes, its carried type
    /// retyped to the parameter's declared type; or the caller's own module parameter `caller`,
    /// passed on bare, its two names. `None` where the retype targets a scheme.
    fn module_terms(
        &mut self,
        caller: Segment,
        (segment, site): (Segment, Site),
        fixed: Fixed,
    ) -> Option<[(TermId, Option<NameId>); 2]> {
        match fixed {
            Fixed::Content(Content::Module {
                content,
                at,
                declared,
                ..
            }) => {
                let DeclaredType::Type(carried) =
                    retype_target(self.types, self.scratch, at.into(), declared)
                else {
                    return None;
                };
                Some([
                    (
                        self.term(GraphTerm::Carried {
                            typed: Interval::point(carried.into()),
                            mentions: &[],
                            variables: &[],
                        }),
                        None,
                    ),
                    (
                        self.term(GraphTerm::Module {
                            content,
                            segment,
                            site,
                        }),
                        None,
                    ),
                ])
            }
            Fixed::Content(Content::Type(_)) => None,
            Fixed::Passed(from) => {
                let unknown = Unknown {
                    segment: caller,
                    at: Address::Slot(from),
                };
                let names = [NameKind::Carried, NameKind::Content]
                    .map(|kind| self.site_name(unknown, kind, KType::ANY));
                Some(names.map(|name| (self.term(GraphTerm::Name(name)), Some(name))))
            }
        }
    }

    /// Where the use `question` sits at is written.
    fn written(&self, question: &SiteQuestion<'_>) -> Option<SourceRef> {
        match self
            .shapes
            .shape_at(question.segment)
            .located(question.site)
        {
            crate::scope::Located::Node(node) => Some(node.source),
            crate::scope::Located::Part(_) => None,
        }
    }

    fn term(&mut self, term: GraphTerm<'g>) -> TermId {
        self.terms.push(term);
        TermId(self.terms.len() as u32 - 1)
    }

    /// What `candidate`'s call, written in `segment`, maps its callee's names to, beside the names
    /// the mapping reads: `None` where it calls no callable body of the text.
    fn mapping(
        &mut self,
        segment: Segment,
        site: Site,
        candidate: &Selectable<'_>,
    ) -> Option<Mapped<'x>> {
        let passes = candidate.passes?;
        let callee = self.body(passes.callee)?;
        let (mapping, reads) = self.slots_mapped(
            (segment, site),
            passes.callee,
            passes.slots,
            candidate.points,
        );
        Some((callee, mapping, reads))
    }

    /// What a call written at `site` of `segment` into `callee` maps its names to, slot by slot,
    /// beside the names the mapping reads: each type name its entry binds, and each module
    /// parameter's carried and content names. A point passes where `points`, the selected
    /// candidate's, solves it closed.
    fn slots_mapped(
        &mut self,
        (segment, site): (Segment, Site),
        callee: Segment,
        slots: &[(Slot, Passing)],
        points: Option<&[PointAsk]>,
    ) -> (BumpVec<'x, (NameId, TermId)>, BumpVec<'x, NameId>) {
        let caller = self.shapes.owner(segment);
        let (mut mapping, mut reads) =
            (BumpVec::new_in(self.scratch), BumpVec::new_in(self.scratch));
        for (slot, passing) in slots {
            if let Passing::Carried(typed) = *passing {
                if let Some(name) = self.entry_carried(callee, *slot)
                    && let Some((term, read)) = self.carried_term(segment, typed)
                {
                    mapping.push((name, term));
                    for name in read.iter() {
                        if !reads.contains(name) {
                            reads.push(*name);
                        }
                    }
                }
                continue;
            }
            let Some(name) = self.entry_name(callee, *slot) else {
                if let Passing::Fixed(fixed) = *passing
                    && let Some((carried, content)) = self.entry_module(callee, *slot)
                    && let Some([carried_term, content_term]) =
                        self.module_terms(caller, (segment, site), fixed)
                {
                    for (name, (term, read)) in [(carried, carried_term), (content, content_term)] {
                        mapping.push((name, term));
                        reads.extend(read.filter(|read| !reads.contains(read)));
                    }
                }
                continue;
            };
            let term = match *passing {
                Passing::Type(ty) => self.type_term(segment, ty.into()),
                Passing::Point(at) => match points.map(|points| points[usize::from(at)]) {
                    Some(point) if point.carried.is_none() => {
                        self.type_term(segment, point.upper.into())
                    }
                    _ => None,
                },
                Passing::Fixed(fixed) => self.fixed_term(caller, fixed),
                Passing::Carried(_) => None,
            };
            if let Some((term, read)) = term {
                mapping.push((name, term));
                for name in read.iter() {
                    if !reads.contains(name) {
                        reads.push(*name);
                    }
                }
            }
        }
        (mapping, reads)
    }

    /// Lays `question`, sitting at `site`: no question where the text decides it, its taken arm's
    /// call passed at the site; else a question over the names its asks read, its candidates'
    /// calls on their arms; and left out where it reads a name no binding writes in.
    fn question(&mut self, site: GraphSite, question: &'g SiteQuestion<'g>) {
        let mut rigid = Rigid {
            question,
            open: false,
        };
        let taken = decide(question, &mut rigid);
        let candidates = match question.arms {
            [Payload::Selection(_), candidates @ ..] if question.rule == SiteRule::Selection => {
                candidates
            }
            _ => &[],
        };
        if !rigid.open {
            if let Some(candidate) = taken.arm().and_then(|arm| arms_candidate(candidates, arm))
                && let Some((callee, mapping, _)) =
                    self.mapping(question.segment, question.site, candidate)
                && let Some(passes) = candidate.passes
            {
                let edge = self.text.passes_at(site, callee, mapping.to_vec());
                self.calls.push(LaidCall {
                    callee,
                    written: self.written(question),
                    held: Some(HeldCall {
                        edge,
                        at: (question.segment, question.site),
                        on: None,
                        into: passes.callee,
                    }),
                });
            }
            return;
        }
        let mut reads = BumpVec::new_in(self.scratch);
        let mut asks = BumpVec::with_capacity_in(question.asks.len(), self.scratch);
        for posed in question.asks {
            if posed.rigid.is_some() {
                asks.push(None);
                continue;
            }
            match self.posed(question.segment, site, posed.question, &mut reads) {
                Some(ask) => asks.push(ask),
                None => {
                    self.left += 1;
                    return;
                }
            }
        }
        let mut calls = BumpVec::new_in(self.scratch);
        for (at, candidate) in candidates.iter().enumerate() {
            let Payload::Candidate(candidate) = candidate else {
                continue;
            };
            if let Some((callee, mapping, read)) =
                self.mapping(question.segment, question.site, candidate)
                && let Some(passes) = candidate.passes
            {
                for name in read.iter() {
                    if !reads.contains(name) {
                        reads.push(*name);
                    }
                }
                calls.push((at as u32 + 1, callee, mapping, passes.callee));
            }
        }
        // Reading no name its chain binds, the question decides at every binding as at the load,
        // and each candidate's call passes what no binding changes.
        if reads.is_empty() {
            return;
        }
        let arms = (question.arms.len() as u32).max(4);
        let id = self.text.question(site, &reads, arms);
        for ask in asks.iter().flatten() {
            self.text.may_ask(id, self.asks[ask.0 as usize].names, *ask);
        }
        for (arm, callee, mapping, into) in calls.iter() {
            let edge = self.text.passes(id, *arm, *callee, mapping.to_vec());
            self.calls.push(LaidCall {
                callee: *callee,
                written: self.written(question),
                held: Some(HeldCall {
                    edge,
                    at: (question.segment, question.site),
                    on: Some((id, *arm)),
                    into: *into,
                }),
            });
        }
        self.questions.push(GraphQuestion::Site {
            question,
            asks: collect(self.lay, asks.iter().copied()),
        });
        let key = (question.segment, question.site, question.rule);
        self.sites.push((key, id));
    }

    /// The ask `question`, posed in `segment` at `site`, over the names it reads on the site's
    /// chain, each gathered into `reads` too: `Some(None)` where it reads none the chain binds,
    /// so it is answered as at the load, and `None` where it reads a name no binding writes in.
    fn posed(
        &mut self,
        segment: Segment,
        site: GraphSite,
        question: Question<'g>,
        reads: &mut BumpVec<'_, NameId>,
    ) -> Option<Option<AskId>> {
        let (mut read, mut through) =
            (BumpVec::new_in(self.scratch), BumpVec::new_in(self.scratch));
        let mut mapped = true;
        question.each_type(&mut |ty| {
            mapped = mapped && self.reads(segment, ty, &mut read, Some(&mut through));
        });
        if !mapped {
            return None;
        }
        if read.is_empty() && !through.iter().any(|name| self.visible(*name, site)) {
            return Some(None);
        }
        let mut names = BumpVec::new_in(self.scratch);
        let mut variables = BumpVec::with_capacity_in(read.len(), self.scratch);
        for (name, variable) in read.iter() {
            let at = match names.iter().position(|held| held == name) {
                Some(at) => at,
                None => {
                    names.push(*name);
                    names.len() - 1
                }
            };
            variables.push((at as u16, *variable));
        }
        // A name of another body's chain is no binding's the question's instance sees: the
        // class reads it as the load does.
        for name in through.iter() {
            if self.visible(*name, site) {
                push_name(&mut names, *name);
            }
        }
        for name in names.iter() {
            push_name(reads, *name);
        }
        self.asks.push(GraphAsk {
            question,
            segment,
            names: collect(self.lay, names.iter().copied()),
            variables: collect(self.lay, variables.iter().copied()),
        });
        Some(Some(AskId(self.asks.len() as u32 - 1)))
    }

    /// What the decided derived name `id`, read in `segment` under `upper`, is to the graph: an
    /// answered name, its deciding question laid at the segment's closing, each arm's type the
    /// content an `AnswersName` edge on that arm gives it. It is constant where neither the
    /// question nor an arm reads a name the chain binds, and read through its question and arms
    /// where an arm is written over a path class, which no term spells. `None` where its question
    /// reads a name no binding writes in.
    fn answered(
        &mut self,
        segment: Segment,
        (id, upper): (DerivedId, KType),
        question: Question<'g>,
        arms: &'g [Parametric],
    ) -> Option<Answered> {
        if let Some(&held) = self.answered.get(&(segment, id)) {
            return Some(held);
        }
        let mut reads = BumpVec::new_in(self.scratch);
        let mut contents = BumpVec::with_capacity_in(arms.len(), self.scratch);
        for arm in arms {
            let Some((content, read)) = self.type_term(segment, (*arm).into()) else {
                self.answered.insert((segment, id), Answered::Through);
                return Some(Answered::Through);
            };
            for name in read.iter() {
                push_name(&mut reads, *name);
            }
            contents.push(content);
        }
        let site = self.closing_site(segment);
        let ask = self.posed(segment, site, question, &mut reads)?;
        let answered = if reads.is_empty() {
            Answered::Constant
        } else {
            let name = self.text.answered_name();
            self.names.push(GraphName {
                address: NameAddress::Answered(id),
                kind: NameKind::Type,
                upper,
            });
            let decides = self.text.question(site, &reads, arms.len() as u32);
            if let Some(ask) = ask {
                self.text
                    .may_ask(decides, self.asks[ask.0 as usize].names, ask);
            }
            self.questions
                .push(GraphQuestion::Decides { question, ask });
            for (arm, content) in contents.iter().enumerate() {
                self.text.answers(decides, arm as u32, name, *content);
            }
            Answered::Name(name)
        };
        self.answered.insert((segment, id), answered);
        Some(answered)
    }
}

/// The site of the graph the unit `unit` of `segment` sits at, `bodies` each owning body's id by
/// segment: an inline body's unit at its own statement ([`GraphFacts::inline`]), or, in an inline
/// body no laid unit runs, at the unit of its encloser that runs it.
fn site_of(
    shapes: &Shapes<'_>,
    bodies: &[Option<BodyId>],
    before: &[(BodyId, u32)],
    inline: &[((Segment, UnitIndex), GraphSite)],
    mut segment: Segment,
    mut unit: UnitIndex,
) -> GraphSite {
    while let ShapeKind::Block | ShapeKind::Module = shapes.shape_at(segment).kind() {
        if let Ok(at) = inline.binary_search_by_key(&(segment, unit), |(held, _)| *held) {
            return inline[at].1;
        }
        let running = (shapes.birth_unit(segment)).expect("an inline body has an encloser");
        (segment, unit) = (running.segment, running.unit);
    }
    let body = bodies[segment.index() as usize].expect("an owning body is laid");
    match unit.run_index() {
        None => GraphSite::Entry(body),
        // After every statement laid before it or an earlier unit.
        Some(index) => {
            let index = index as u32;
            let held = before.partition_point(|at| *at <= (body, index))
                - before.partition_point(|(each, _)| *each < body);
            GraphSite::Statement(body, index + held as u32)
        }
    }
}

/// Gather `name` into `names`, once.
fn push_name(names: &mut BumpVec<'_, NameId>, name: NameId) {
    if !names.contains(&name) {
        names.push(name);
    }
}

/// The candidate a selection question's arm `arm` holds: its arms past the layout are its
/// candidates', then its refusals.
fn arms_candidate<'a>(candidates: &[Payload<'a>], arm: u16) -> Option<&'a Selectable<'a>> {
    match candidates.get(usize::from(arm).checked_sub(1)?) {
        Some(Payload::Candidate(candidate)) => Some(candidate),
        _ => None,
    }
}

/// Each of `names` that `term` mentions, by index, beside how: **bare** where the term is the name
/// itself or a rule that only takes a part of what it reads, an **alternative** where it is one
/// member of a union the term builds, and **built** everywhere else. A name mentioned more than
/// one way is classed by the deepest: built over bare over alternative. `terms` finds a derived
/// name's term.
pub fn mentions<'x, 'n>(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'x>,
    terms: &dyn Fn(DerivedId) -> Option<DerivedName<'n>>,
    term: DeclaredType<Parametric>,
    names: &[NameVariable],
) -> BumpVec<'x, (usize, Nesting)> {
    let mut found = BumpVec::new_in(scratch);
    let mut note =
        |index: usize, nesting: Nesting| match found.iter_mut().find(|(held, _)| *held == index) {
            Some((_, held)) if depth_rank(nesting) > depth_rank(*held) => *held = nesting,
            Some(_) => {}
            None => found.push((index, nesting)),
        };
    classed(types, scratch, terms, term, names, Nesting::Bare, &mut note);
    found.sort_by_key(|(index, _)| *index);
    found
}

/// The order mentions are classed by: an alternative adds the least.
fn depth_rank(nesting: Nesting) -> u8 {
    match nesting {
        Nesting::Alternative => 0,
        Nesting::Bare => 1,
        Nesting::Built => 2,
    }
}

/// Note each of `names` `ty` mentions, `at` the nesting the position it lies at gives: bare for
/// the term itself or a part-taking rule's operand, built under a type the term builds.
fn classed<'n>(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    terms: &dyn Fn(DerivedId) -> Option<DerivedName<'n>>,
    ty: DeclaredType<Parametric>,
    names: &[NameVariable],
    at: Nesting,
    note: &mut dyn FnMut(usize, Nesting),
) {
    let DeclaredType::Type(typed) = ty else {
        return every(types, scratch, terms, ty, names, Nesting::Built, note);
    };
    if types.concrete(typed).is_some() {
        return;
    }
    match types.node(typed) {
        TypeNode::Lexical { level, .. } => {
            if let Some(index) = position_of(names, NameVariable::Level(level)) {
                note(index, at);
            }
        }
        TypeNode::Derived { term, .. } => {
            if let Some(index) = position_of(names, NameVariable::Derived(term)) {
                return note(index, at);
            }
            let Some(name) = terms(term) else {
                return;
            };
            match name.term {
                DerivedTerm::Output { rule, operands, .. } if takes_a_part(rule) => {
                    for operand in operands {
                        classed(types, scratch, terms, *operand, names, at, note);
                    }
                }
                _ => every(types, scratch, terms, ty, names, Nesting::Built, note),
            }
        }
        TypeNode::Union { members } => {
            for member in members.iter() {
                let alternative = match types.node(member) {
                    TypeNode::Lexical { level, .. } => {
                        position_of(names, NameVariable::Level(level))
                    }
                    TypeNode::Derived { term, .. } => {
                        position_of(names, NameVariable::Derived(term))
                    }
                    _ => None,
                };
                match alternative {
                    Some(index) => note(index, Nesting::Alternative),
                    None => classed(types, scratch, terms, member.into(), names, at, note),
                }
            }
        }
        _ => every(types, scratch, terms, ty, names, Nesting::Built, note),
    }
}

/// Note every one of `names` `ty` reads, itself or through a derived name's term, `as` the
/// nesting given: built under a type the term builds, bare where it only decides.
fn every<'n>(
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
    terms: &dyn Fn(DerivedId) -> Option<DerivedName<'n>>,
    ty: DeclaredType<Parametric>,
    names: &[NameVariable],
    nesting: Nesting,
    note: &mut dyn FnMut(usize, Nesting),
) {
    for variable in free_variables(types, scratch, ty).iter() {
        if let Some(index) = NameVariable::of(*variable).and_then(|name| position_of(names, name)) {
            note(index, nesting);
        } else if let Variable::Derived { term, .. } = *variable
            && let Some(name) = terms(term)
        {
            name.term.each_type(&mut |operand| {
                every(types, scratch, terms, operand, names, nesting, note);
            });
        }
    }
}

fn position_of(names: &[NameVariable], name: NameVariable) -> Option<usize> {
    names.iter().position(|held| *held == name)
}

/// Whether `rule`'s output is a part of its first operand, never deeper than it: a piece, a member
/// or variant read, a payload, a field read, or a signature's value member.
fn takes_a_part(rule: Rule) -> bool {
    matches!(
        rule,
        Rule::Piece(_)
            | Rule::Member(_)
            | Rule::Variant(_)
            | Rule::Payload(_)
            | Rule::FieldAbove(_)
            | Rule::FieldUnder(_)
            | Rule::SignatureValue(_)
            | Rule::Meet
    )
}

/// Whether `tree` holds the leaf of a call only an answer selects among.
fn holds_call(tree: Tree<'_>) -> bool {
    let mut held = matches!(tree, Tree::Leaf(Leaf::Call { .. }));
    tree.subtrees(&mut |subtree| held = held || holds_call(*subtree));
    held
}
