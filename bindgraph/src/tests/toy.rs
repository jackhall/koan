//! The toy domain, its answer function, the plain-data texts the laws generate, what the laws
//! compute from a text alone, and the growth oracle.
//!
//! A name is bound to a [`Term`]: lists nested over an atom or an open name, or a union of such
//! terms. A term is an id into a thread-local table of hash-consed [`Node`]s, so it is `Copy` and
//! its id is its digest; a term mentioning no name lasts. A `NameQuestion` is a [`Probe`] of a
//! term's depth, its leaf, which name it is where it is a bare name, or whether two terms are the
//! same, answered as a [`Span`] that an open name leaves wide. A span is counted: its bounds are at most 255, or unbounded
//! above. A union is flat, in id order and without repeats ([`union`]), so one set of alternatives
//! is one term. The union of none, `Never`, is no name's content, and a name bound to it drops out
//! of a union it is an alternative of. A `Depth` or `Leaf` probe of a union spans its
//! alternatives'. An answered name's rigid is the bare name itself, so `Which` tells rigids apart.
//! A question's answer adds up the spans its plan asks for, and takes the arm its total picks once
//! the total is exact. `Pair` and `Peel` appear only in hand-written cases, never in generated
//! texts, so each alternative of a generated term mentions at most one name and
//! [`grows_by_terms`] stays exact.
//!
//! A [`ToyText`] is plain data the generators draw from a stream of raw integers ([`Draw`]),
//! fixing each choice up by taking it modulo the size of what it picks from. Each layer of a text,
//! and the bindings after it, draws from its own stretch of the stream ([`Draw::split`]), so a
//! change to one layer moves no other's draws. Knots come from loops drawn whole
//! ([`tail_loop`]), shaped after the hand-written knot cases. Shrinking the raw stream toward zero
//! shrinks the text. [`bases`], [`linear_order`] and [`placed`] restate the graph's bases, site
//! order and placement from the plain text, independently of the crate.

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::fmt;

use proptest::prelude::*;

use crate::{
    Answers, Binding, BodyId, Doors, EdgeId, Facts, NameId, Nesting, QuestionId, Site, Text,
};

/// A name's content, and a term over the caller's names: an id into this thread's table.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct Term(u32);

/// What a term is.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub(super) enum Node {
    Atom(u8),
    List(Term),
    /// A name, open where it appears.
    Name(NameId),
    /// Two terms side by side, so a mapping can mention two names. Cases only.
    Pair(Term, Term),
    /// The element of a list: a part of a name, reported bare. Cases only.
    Peel(Term),
    /// A union: an alternative and the rest, kept flat and in id order by [`union`].
    Union(Term, Term),
    /// The union of none: no name's content.
    Never,
}

/// This thread's terms: each node once, beside whether it mentions no name.
#[derive(Default)]
struct Terms {
    nodes: Vec<(Node, bool)>,
    ids: HashMap<Node, Term>,
}

thread_local! {
    static TERMS: RefCell<Terms> = RefCell::new(Terms::default());
}

/// The term that is `node`.
fn intern(node: Node) -> Term {
    let closed = match node {
        Node::Atom(_) | Node::Never => true,
        Node::Name(_) => false,
        Node::List(inner) | Node::Peel(inner) => closed(inner),
        Node::Pair(left, right) | Node::Union(left, right) => closed(left) && closed(right),
    };
    TERMS.with_borrow_mut(|terms| {
        if let Some(&term) = terms.ids.get(&node) {
            return term;
        }
        let term = Term(u32::try_from(terms.nodes.len()).expect("terms fit u32"));
        terms.nodes.push((node, closed));
        terms.ids.insert(node, term);
        term
    })
}

/// What `term` is.
pub(super) fn node(term: Term) -> Node {
    TERMS.with_borrow(|terms| terms.nodes[term.0 as usize].0)
}

/// Whether `term` mentions no name.
pub(super) fn closed(term: Term) -> bool {
    TERMS.with_borrow(|terms| terms.nodes[term.0 as usize].1)
}

impl fmt::Debug for Term {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match node(*self) {
            Node::Atom(atom) => write!(f, "Atom({atom})"),
            Node::List(inner) => write!(f, "List({inner:?})"),
            Node::Name(name) => write!(f, "Name({})", name.index()),
            Node::Pair(left, right) => write!(f, "Pair({left:?}, {right:?})"),
            Node::Peel(inner) => write!(f, "Peel({inner:?})"),
            Node::Union(left, right) => write!(f, "Union({left:?}, {right:?})"),
            Node::Never => write!(f, "Never"),
        }
    }
}

/// `atom` as a term.
pub(super) fn atom(atom: u8) -> Term {
    intern(Node::Atom(atom))
}

/// `List` around `term`.
pub(super) fn list(term: Term) -> Term {
    intern(Node::List(term))
}

/// `name` as a term.
pub(super) fn name(name: NameId) -> Term {
    intern(Node::Name(name))
}

/// `left` and `right` side by side.
pub(super) fn pair(left: Term, right: Term) -> Term {
    intern(Node::Pair(left, right))
}

/// The element of `term`'s list.
pub(super) fn peel(term: Term) -> Term {
    intern(Node::Peel(term))
}

/// The union of `alternatives`: flat, without `Never`, in id order and without repeats, so one
/// set of alternatives is one term. `Never` where none is left, and the one alternative itself
/// where one is.
pub(super) fn union(alternatives: &[Term]) -> Term {
    let mut flat = Vec::new();
    for &alternative in alternatives {
        flatten(alternative, &mut flat);
    }
    flat.sort_unstable();
    flat.dedup();
    let Some((&last, rest)) = flat.split_last() else {
        return intern(Node::Never);
    };
    (rest.iter().rev()).fold(last, |tail, &first| intern(Node::Union(first, tail)))
}

/// `term`'s alternatives into `out`: a union's, none of `Never`, and any other term itself.
fn flatten(term: Term, out: &mut Vec<Term>) {
    match node(term) {
        Node::Union(first, rest) => {
            flatten(first, out);
            flatten(rest, out);
        }
        Node::Never => {}
        _ => out.push(term),
    }
}

/// One judgement about names: how deep a name's lists nest, what its leaf atom is, which name it
/// is where it is a bare name, so a wrong rigid changes an answer, or whether two names hold the
/// same term.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub(super) enum Probe {
    Depth,
    Leaf,
    Which,
    Same,
}

/// An inclusive range; `hi == Span::MAX` is unbounded.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub(super) struct Span {
    pub(super) lo: u32,
    pub(super) hi: u32,
}

impl Span {
    pub(super) const MAX: u32 = u32::MAX;

    pub(super) fn exact(at: u32) -> Self {
        Self { lo: at, hi: at }
    }

    /// The least span covering both.
    fn hull(self, other: Self) -> Self {
        Self {
            lo: self.lo.min(other.lo),
            hi: self.hi.max(other.hi),
        }
    }

    /// The sum of two spans, unbounded where either is.
    fn plus(self, other: Self) -> Self {
        let hi = if self.hi == Self::MAX || other.hi == Self::MAX {
            Self::MAX
        } else {
            self.hi.saturating_add(other.hi)
        };
        Self {
            lo: self.lo.saturating_add(other.lo),
            hi,
        }
    }
}

/// A question's answer: the total span its plan added, and the arm an exact total picks.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(super) struct Verdict {
    pub(super) span: Span,
    pub(super) arm: Option<u32>,
}

/// The toy domain.
pub(super) struct Toy;

/// What `probe`, `Depth` or `Leaf`, answers of `term`: a list adds one to the depth, a name or
/// a part of one is any depth from there and any leaf, and a union spans its alternatives.
fn span_of(probe: Probe, term: Term) -> Span {
    match (probe, node(term)) {
        (Probe::Depth, Node::List(inner)) => span_of(probe, inner).plus(Span::exact(1)),
        (Probe::Leaf, Node::List(inner)) => span_of(probe, inner),
        (_, Node::Union(first, rest)) => span_of(probe, first).hull(span_of(probe, rest)),
        (_, Node::Never) => unreachable!("the empty union is no name's content"),
        (Probe::Depth, Node::Atom(_) | Node::Pair(..)) => Span::exact(0),
        (Probe::Depth, Node::Name(_) | Node::Peel(_)) => Span {
            lo: 0,
            hi: Span::MAX,
        },
        (Probe::Leaf, Node::Atom(atom)) => Span::exact(u32::from(atom)),
        (Probe::Leaf, _) => Span { lo: 0, hi: 255 },
        (Probe::Which | Probe::Same, _) => unreachable!("answered in `ask`"),
    }
}

/// `term` with each bound name replaced, or `None` where a `Peel` meets open content.
fn substitute(term: Term, binding: &Binding<Toy>) -> Option<Term> {
    Some(match node(term) {
        Node::Atom(_) => term,
        Node::List(inner) => list(substitute(inner, binding)?),
        Node::Name(name) => binding.get(name).unwrap_or(term),
        Node::Pair(left, right) => pair(substitute(left, binding)?, substitute(right, binding)?),
        Node::Peel(inner) => match node(substitute(inner, binding)?) {
            Node::List(element) => element,
            Node::Atom(leaf) => atom(leaf),
            _ => return None,
        },
        Node::Union(first, rest) => {
            union(&[substitute(first, binding)?, substitute(rest, binding)?])
        }
        Node::Never => term,
    })
}

/// Each name `term` mentions, where a name here is mentioned as `nesting` says: under a `List`
/// it is built, as an alternative of a union it is one alternative, and inside a `Pair` or a
/// `Peel` that is an alternative it is a part, so bare.
fn mentions(term: Term, nesting: Nesting, out: &mut Vec<(NameId, Nesting)>) {
    let part = match nesting {
        Nesting::Alternative => Nesting::Bare,
        other => other,
    };
    match node(term) {
        Node::Atom(_) | Node::Never => {}
        Node::List(inner) => mentions(inner, Nesting::Built, out),
        Node::Name(name) => out.push((name, nesting)),
        Node::Pair(left, right) => {
            mentions(left, part, out);
            mentions(right, part, out);
        }
        Node::Peel(inner) => mentions(inner, part, out),
        Node::Union(first, rest) => {
            let alternative = match nesting {
                Nesting::Built => Nesting::Built,
                _ => Nesting::Alternative,
            };
            mentions(first, alternative, out);
            mentions(rest, alternative, out);
        }
    }
}

/// A span's upper bound as counted: 0 to 255, or 256 for unbounded.
const BOUNDS: u32 = 257;

impl Facts for Toy {
    type Term = Term;
    type NameQuestion = Probe;
    type NameAnswer = Span;
    type SiteAnswer = Verdict;

    fn mentions(&self, term: Term) -> Vec<(NameId, Nesting)> {
        let mut out = Vec::new();
        mentions(term, Nesting::Bare, &mut out);
        out
    }

    fn arm(&self, answer: Verdict) -> Option<u32> {
        answer.arm
    }

    fn answer_count(&self, _: Probe) -> u32 {
        256 * BOUNDS
    }

    fn answer_index(&self, _: Probe, span: Span) -> u32 {
        assert!(
            span.lo <= 255 && (span.hi <= 255 || span.hi == Span::MAX),
            "{span:?} is past the toy's count"
        );
        span.lo * BOUNDS + if span.hi == Span::MAX { 256 } else { span.hi }
    }

    fn nth_answer(&self, _: Probe, index: u32) -> Span {
        let hi = index % BOUNDS;
        Span {
            lo: index / BOUNDS,
            hi: if hi == 256 { Span::MAX } else { hi },
        }
    }
}

impl crate::Domain for Toy {
    type Facts = Toy;
    type Content = Term;

    fn ask(&self, question: Probe, contents: &[Option<Term>]) -> Span {
        assert!(
            contents
                .iter()
                .flatten()
                .all(|&content| node(content) != Node::Never),
            "the empty union is no name's content"
        );
        let unbounded = Span {
            lo: 0,
            hi: Span::MAX,
        };
        if question == Probe::Same {
            let [left, right] = contents else {
                panic!("`Same` asks of two names")
            };
            return match (left, right) {
                (Some(left), Some(right)) if closed(*left) && closed(*right) => {
                    Span::exact(u32::from(left == right))
                }
                // Open, or mentioning a name: unrelated unknowns.
                _ => Span { lo: 0, hi: 1 },
            };
        }
        let [content] = contents else {
            panic!("{question:?} asks of one name")
        };
        if question == Probe::Which {
            return match content.map(node) {
                Some(Node::Name(name)) => Span::exact(name.index() as u32),
                _ => unbounded,
            };
        }
        let Some(term) = *content else {
            return match question {
                Probe::Leaf => Span { lo: 0, hi: 255 },
                _ => unbounded,
            };
        };
        span_of(question, term)
    }

    fn apply(&self, term: Term, binding: &Binding<Self>) -> Option<Term> {
        substitute(term, binding)
    }

    fn union(&self, alternatives: &[Term]) -> Term {
        union(alternatives)
    }

    fn rigid(&self, name: NameId) -> Term {
        self::name(name)
    }

    fn digest(&self, content: Term) -> u128 {
        u128::from(content.0)
    }

    fn lasts(&self, content: Term) -> bool {
        closed(content)
    }
}

/// One step of a question's plan: probe read names, perhaps only where the step before allows.
#[derive(Clone, Debug)]
pub(super) struct Step {
    pub(super) names: Vec<NameId>,
    pub(super) probe: Probe,
    pub(super) guarded: bool,
}

/// The toy answer function: per question, its plan and how many arms it has.
#[derive(Clone, Debug, Default)]
pub(super) struct ToyAnswers {
    pub(super) plans: Vec<(Vec<Step>, u32)>,
}

impl Answers<Toy> for ToyAnswers {
    /// Walks the plan, adding each step's span. A guarded step looks at the span the step before
    /// added: it is skipped where that is zero, asked and added where it is at least one, and asked
    /// and added from zero up where it is undecided. So instances of one node may ask different
    /// `NameQuestion`s, and an answer only narrows as more is bound.
    fn answer(&mut self, question: QuestionId, doors: &mut Doors<'_, Toy>) -> Verdict {
        let (plan, arms) = &self.plans[question.index()];
        let mut total = Span::exact(0);
        let mut previous: Option<Span> = None;
        for step in plan {
            let added = match previous {
                Some(before) if step.guarded && before.hi == 0 => Span::exact(0),
                Some(before) if step.guarded && before.lo == 0 => Span {
                    lo: 0,
                    hi: doors.ask(&step.names, step.probe).hi,
                },
                _ => doors.ask(&step.names, step.probe),
            };
            total = total.plus(added);
            previous = Some(added);
        }
        let arm = (*arms > 0 && total.lo == total.hi).then(|| total.lo % arms);
        Verdict { span: total, arm }
    }
}

/// A toy site: a body and its statement, or `None` for its entry.
pub(super) type ToySite = (usize, Option<u32>);

/// A question as plain data.
#[derive(Clone, Debug)]
pub(super) struct ToyQuestion {
    pub(super) site: ToySite,
    pub(super) reads: Vec<usize>,
    pub(super) plan: Vec<Step>,
    pub(super) arms: u32,
}

/// What selects a call, as plain data.
#[derive(Copy, Clone, Debug)]
pub(super) enum ToyOn {
    Arm { question: usize, arm: u32 },
    Site(ToySite),
}

/// A call as plain data.
#[derive(Clone, Debug)]
pub(super) struct ToyEdge {
    pub(super) on: ToyOn,
    pub(super) callee: usize,
    pub(super) mapping: Vec<(usize, Term)>,
}

/// Where an `AnswersName` edge takes its content from, as plain data.
#[derive(Clone, Debug)]
pub(super) enum ToySource {
    Arm {
        question: usize,
        arm: u32,
        content: Term,
    },
    Call {
        edge: usize,
        takes: usize,
    },
    Site {
        site: ToySite,
        content: Term,
    },
}

/// An `AnswersName` edge as plain data.
#[derive(Clone, Debug)]
pub(super) struct ToyAnswer {
    pub(super) name: usize,
    pub(super) source: ToySource,
}

/// A text as plain data. Names, questions, calls and `AnswersName` edges are numbered in order,
/// so index `i` here is the id `i` in the built text. The site names come first; answered name
/// `k` is `NameId` `names.len() + k`.
#[derive(Clone, Debug, Default)]
pub(super) struct ToyText {
    /// Per body, its encloser (ignored for the root) and its statement count.
    pub(super) bodies: Vec<(usize, u32)>,
    pub(super) names: Vec<ToySite>,
    pub(super) answered: usize,
    pub(super) questions: Vec<ToyQuestion>,
    pub(super) edges: Vec<ToyEdge>,
    pub(super) answers: Vec<ToyAnswer>,
    pub(super) open_callers: Vec<usize>,
}

/// The crate's site for a toy site.
pub(super) fn site((body, statement): ToySite) -> Site {
    let body = BodyId::new(body);
    statement.map_or(Site::Entry(body), |index| Site::Statement(body, index))
}

impl ToyText {
    /// The text described, and its answer function.
    pub(super) fn build(&self) -> (Text<Toy>, ToyAnswers) {
        let mut text = Text::new();
        for (index, &(encloser, statements)) in self.bodies.iter().enumerate() {
            let body = if index == 0 {
                text.root()
            } else {
                text.body(BodyId::new(encloser))
            };
            for _ in 0..statements {
                text.statement(body);
            }
        }
        for &at in &self.names {
            text.name(site(at));
        }
        for _ in 0..self.answered {
            text.answered_name();
        }
        let mut answers = ToyAnswers::default();
        for question in &self.questions {
            let reads: Vec<NameId> = question
                .reads
                .iter()
                .map(|&read| NameId::new(read))
                .collect();
            let id = text.question(site(question.site), &reads, question.arms);
            for step in &question.plan {
                text.may_ask(id, &step.names, step.probe);
            }
            answers.plans.push((question.plan.clone(), question.arms));
        }
        for edge in &self.edges {
            let mapping = edge
                .mapping
                .iter()
                .map(|&(name, term)| (NameId::new(name), term));
            let callee = BodyId::new(edge.callee);
            match edge.on {
                ToyOn::Arm { question, arm } => {
                    text.passes(QuestionId::new(question), arm, callee, mapping.collect())
                }
                ToyOn::Site(at) => text.passes_at(site(at), callee, mapping.collect()),
            };
        }
        for answer in &self.answers {
            let name = NameId::new(answer.name);
            match &answer.source {
                ToySource::Arm {
                    question,
                    arm,
                    content,
                } => text.answers(QuestionId::new(*question), *arm, name, *content),
                ToySource::Call { edge, takes } => {
                    text.answers_through(EdgeId::new(*edge), name, NameId::new(*takes))
                }
                ToySource::Site { site: at, content } => text.answers_at(site(*at), name, *content),
            };
        }
        for &body in &self.open_callers {
            text.open_caller(BodyId::new(body));
        }
        (text, answers)
    }

    /// The names bound at `body`'s entry.
    pub(super) fn entry_names(&self, body: usize) -> Vec<usize> {
        (0..self.names.len())
            .filter(|&name| self.names[name] == (body, None))
            .collect()
    }

    /// Whether `ancestor` is a proper ancestor of `body`.
    pub(super) fn encloses(&self, ancestor: usize, mut body: usize) -> bool {
        while body != 0 {
            body = self.bodies[body].0;
            if body == ancestor {
                return true;
            }
        }
        false
    }

    /// The names a question at `at` may read: bound in a proper ancestor, or earlier in its body.
    fn visible(&self, at: ToySite) -> Vec<usize> {
        let rank = |statement: Option<u32>| statement.map_or(0, |index| index + 1);
        (0..self.names.len())
            .filter(|&name| {
                let (body, statement) = self.names[name];
                if body == at.0 {
                    rank(statement) <= rank(at.1)
                } else {
                    self.encloses(body, at.0)
                }
            })
            .collect()
    }
}

/// The site names each name's content depends on, each call's inputs, and each body's captures,
/// computed from the plain text alone. Sets hold site-name indices.
pub(super) struct Bases {
    pub(super) names: Vec<BTreeSet<usize>>,
    pub(super) inputs: Vec<BTreeSet<usize>>,
    pub(super) captured: Vec<BTreeSet<usize>>,
}

/// The indices of the names `term` mentions.
fn mentioned(term: Term) -> Vec<usize> {
    Toy.mentions(term)
        .into_iter()
        .map(|(name, _)| name.index())
        .collect()
}

/// Whether a name bound at `bound` is visible at `at`.
pub(super) fn sees(text: &ToyText, bound: ToySite, at: ToySite) -> bool {
    let rank = |statement: Option<u32>| statement.map_or(0, |index| index + 1);
    if bound.0 == at.0 {
        rank(bound.1) <= rank(at.1)
    } else {
        text.encloses(bound.0, at.0)
    }
}

/// The site a call sits at.
pub(super) fn call_site(text: &ToyText, edge: &ToyEdge) -> ToySite {
    match edge.on {
        ToyOn::Arm { question, .. } => text.questions[question].site,
        ToyOn::Site(at) => at,
    }
}

/// [`Bases`] by repeating every rule until nothing grows: a site name's base is itself, an
/// answered name's the union of what each edge typing it depends on, a call's inputs its
/// mentions, its question's reads and what its callee captures that its site sees, and a body's
/// captures the site names bound outside it in what a reader inside it waits on.
pub(super) fn bases(text: &ToyText) -> Bases {
    let sites = text.names.len();
    let mut bases = Bases {
        names: (0..sites + text.answered)
            .map(|name| {
                if name < sites {
                    BTreeSet::from([name])
                } else {
                    BTreeSet::new()
                }
            })
            .collect(),
        inputs: vec![BTreeSet::new(); text.edges.len()],
        captured: vec![BTreeSet::new(); text.bodies.len()],
    };
    let of = |bases: &Bases, names: &[usize]| -> BTreeSet<usize> {
        names
            .iter()
            .flat_map(|&name| bases.names[name].iter().copied())
            .collect()
    };
    let within = |ancestor: usize, body: usize| ancestor == body || text.encloses(ancestor, body);
    loop {
        let before = (
            bases.names.iter().map(BTreeSet::len).sum::<usize>(),
            bases.inputs.iter().map(BTreeSet::len).sum::<usize>(),
            bases.captured.iter().map(BTreeSet::len).sum::<usize>(),
        );
        for (index, edge) in text.edges.iter().enumerate() {
            let mentioned: Vec<usize> = (edge.mapping.iter())
                .flat_map(|&(_, term)| Toy.mentions(term))
                .map(|(name, _)| name.index())
                .collect();
            let mut inputs = of(&bases, &mentioned);
            if let ToyOn::Arm { question, .. } = edge.on {
                inputs.extend(of(&bases, &text.questions[question].reads));
            }
            let at = call_site(text, edge);
            for &name in &bases.captured[edge.callee] {
                if sees(text, text.names[name], at) {
                    inputs.insert(name);
                }
            }
            bases.inputs[index].extend(inputs);
        }
        for answer in &text.answers {
            let depends = match &answer.source {
                ToySource::Arm { question, .. } => of(&bases, &text.questions[*question].reads),
                ToySource::Call { edge, .. } => bases.inputs[*edge].clone(),
                ToySource::Site { content, .. } => of(&bases, &mentioned(*content)),
            };
            bases.names[answer.name].extend(depends);
        }
        let mut readers: Vec<(usize, BTreeSet<usize>)> = Vec::new();
        for question in &text.questions {
            readers.push((question.site.0, of(&bases, &question.reads)));
        }
        for (index, edge) in text.edges.iter().enumerate() {
            readers.push((call_site(text, edge).0, bases.inputs[index].clone()));
        }
        for answer in &text.answers {
            if let ToySource::Call { edge, takes } = answer.source {
                readers.push((text.edges[edge].callee, bases.names[takes].clone()));
            }
        }
        for answer in &text.answers {
            if let ToySource::Site { site: at, content } = &answer.source {
                readers.push((at.0, of(&bases, &mentioned(*content))));
            }
        }
        for (body, read) in readers {
            let mut at = Some(body);
            while let Some(ancestor) = at {
                for &name in &read {
                    if !within(ancestor, text.names[name].0) {
                        bases.captured[ancestor].insert(name);
                    }
                }
                at = (ancestor != 0).then(|| text.bodies[ancestor].0);
            }
        }
        let after = (
            bases.names.iter().map(BTreeSet::len).sum::<usize>(),
            bases.inputs.iter().map(BTreeSet::len).sum::<usize>(),
            bases.captured.iter().map(BTreeSet::len).sum::<usize>(),
        );
        if after == before {
            return bases;
        }
    }
}

/// Every site of the text in one line: a body's entry, then the bodies inside it that capture
/// none of its statements, then each statement followed by the bodies whose last capture it is.
pub(super) fn linear_order(text: &ToyText, bases: &Bases) -> HashMap<ToySite, usize> {
    fn lay_out(text: &ToyText, bases: &Bases, body: usize, out: &mut Vec<ToySite>) {
        let children = (1..text.bodies.len()).filter(|&child| text.bodies[child].0 == body);
        let last_capture = |child: usize| {
            (bases.captured[child].iter())
                .map(|&name| text.names[name])
                .filter(|&(at, _)| at == body)
                .filter_map(|(_, statement)| statement)
                .max()
        };
        let captures: Vec<(usize, Option<u32>)> =
            children.map(|child| (child, last_capture(child))).collect();
        out.push((body, None));
        let follow = |after: Option<u32>, out: &mut Vec<ToySite>| {
            for &(child, last) in &captures {
                if last == after {
                    lay_out(text, bases, child, out);
                }
            }
        };
        follow(None, out);
        for statement in 0..text.bodies[body].1 {
            out.push((body, Some(statement)));
            follow(Some(statement), out);
        }
    }
    let mut out = Vec::new();
    lay_out(text, bases, 0, &mut out);
    out.into_iter()
        .enumerate()
        .map(|(position, at)| (at, position))
        .collect()
}

/// A stream of raw integers, each turned into one choice.
pub(super) struct Draw {
    raw: Vec<u32>,
    at: usize,
}

impl Draw {
    pub(super) fn new(raw: Vec<u32>) -> Self {
        Self { raw, at: 0 }
    }

    /// A choice in `0..count`, or 0 where `count` is 0 or the stream has run out.
    pub(super) fn pick(&mut self, count: usize) -> usize {
        let raw = self.raw.get(self.at).copied().unwrap_or(0);
        self.at += 1;
        if count == 0 { 0 } else { raw as usize % count }
    }

    /// True with probability `numerator / denominator`.
    pub(super) fn chance(&mut self, numerator: usize, denominator: usize) -> bool {
        self.pick(denominator) < numerator
    }

    /// The next `count` raw integers as a stream of their own, so how many choices one part of a
    /// generator makes moves no later part's.
    pub(super) fn split(&mut self, count: usize) -> Self {
        let start = self.at.min(self.raw.len());
        let end = (self.at + count).min(self.raw.len());
        self.at += count;
        Self::new(self.raw[start..end].to_vec())
    }
}

/// The raw stream a generated text and its bindings are drawn from: the text from the first 640,
/// the bindings from the rest.
pub(super) fn raw() -> impl Strategy<Value = Vec<u32>> {
    prop::collection::vec(any::<u32>(), 1280)
}

/// What a reader at `at` may read: the visible site names, and each answered name of `sources`
/// whose source site the reader sees.
fn readable(text: &ToyText, sources: &[ToySite], at: ToySite) -> Vec<usize> {
    let mut names = text.visible(at);
    for (index, &source) in sources.iter().enumerate() {
        if sees(text, source, at) {
            names.push(text.names.len() + index);
        }
    }
    names
}

/// Any site of the text.
fn any_site(draw: &mut Draw, text: &ToyText) -> ToySite {
    let body = draw.pick(text.bodies.len());
    let at = draw.pick(text.bodies[body].1 as usize + 1);
    (body, at.checked_sub(1).map(|index| index as u32))
}

/// A step of a plan over `reads`: one time in four where it reads two names or more, whether two
/// distinct reads are the same; otherwise a probe of one read.
fn step(draw: &mut Draw, reads: &[usize]) -> Step {
    if reads.len() >= 2 && draw.chance(1, 4) {
        let first = draw.pick(reads.len());
        let second = (first + 1 + draw.pick(reads.len() - 1)) % reads.len();
        let names = vec![NameId::new(reads[first]), NameId::new(reads[second])];
        let guarded = draw.chance(1, 2);
        return Step {
            names,
            probe: Probe::Same,
            guarded,
        };
    }
    Step {
        names: vec![NameId::new(reads[draw.pick(reads.len())])],
        probe: [Probe::Depth, Probe::Leaf, Probe::Which][draw.pick(3)],
        guarded: draw.chance(1, 2),
    }
}

/// A term mentioning at most one of `names`: an atom, a name, or either inside lists.
fn term_over(draw: &mut Draw, names: &[usize]) -> Term {
    let atom = atom(draw.pick(4) as u8);
    if names.is_empty() {
        return if draw.chance(1, 2) { atom } else { list(atom) };
    }
    let read = name_of(names[draw.pick(names.len())]);
    match draw.pick(10) {
        0..=2 => atom,
        3..=6 => read,
        7 => list(read),
        8 => list(list(read)),
        _ => list(atom),
    }
}

/// One alternative of a union over `names`: an atom, one of the names, or one inside a list.
fn alternative(draw: &mut Draw, names: &[usize]) -> Term {
    let leaf = atom(draw.pick(4) as u8);
    if names.is_empty() {
        return leaf;
    }
    let read = name_of(names[draw.pick(names.len())]);
    match draw.pick(8) {
        0 | 1 => leaf,
        2..=6 => read,
        _ => list(read),
    }
}

/// Any body but the root.
fn any_callee(draw: &mut Draw, text: &ToyText) -> usize {
    1 + draw.pick(text.bodies.len() - 1)
}

/// A call on `on` into `callee`, mapping some of its entry names over `names`.
fn call_into(
    draw: &mut Draw,
    text: &ToyText,
    on: ToyOn,
    callee: usize,
    names: &[usize],
) -> ToyEdge {
    let mut mapping = Vec::new();
    for name in text.entry_names(callee) {
        if draw.chance(3, 4) {
            mapping.push((name, term_over(draw, names)));
        }
    }
    ToyEdge {
        on,
        callee,
        mapping,
    }
}

/// A question at `at` with `arms` arms, reading one to three of `readable` through a plan of
/// one to three steps.
fn question_at(draw: &mut Draw, at: ToySite, readable: &[usize], arms: u32) -> ToyQuestion {
    let mut reads = Vec::new();
    for _ in 0..1 + draw.pick(3) {
        let read = readable[draw.pick(readable.len())];
        if !reads.contains(&read) {
            reads.push(read);
        }
    }
    let plan = (0..1 + draw.pick(3)).map(|_| step(draw, &reads)).collect();
    ToyQuestion {
        site: at,
        reads,
        plan,
        arms,
    }
}

/// How a name of a loop's ring is typed: each needs the ring's next name.
#[derive(Copy, Clone, PartialEq, Eq)]
enum Member {
    /// Through a call of the loop out of the name's body, taking the next name in the callee: a
    /// tail call's result.
    Takes,
    /// On a site, by a union of the next name beside a taker of the ring or alternatives from
    /// outside the ring: a `MATCH` with a tail arm.
    Alternative,
    /// On a site, by a union with the next name inside a list: a type built on the knot.
    Built,
    /// On the arms of a question reading the next name: a dispatch on the knot.
    Decided,
}

/// A loop drawn whole, where a knot can form, after the hand-written knot cases. Mostly in one
/// body `L` other than the root, with one or two self-calls at one site of `L`, mostly passing
/// `L`'s entry names on unchanged. A third of the time, where there is a second such body `M`,
/// in `L` and `M`, each calling the other at one site, mostly passing an entry name bare: a knot
/// across two bodies. Then a ring of one to three answered names, each needing the next, at
/// least one through a call. A taker lies in its call's body and takes from the callee, and any
/// other name lies in its next name's body at or after it, so the ring closes only through a
/// take. Half the time, a call into `L` from any site, half of those on an arm of a new
/// question, types a name taking one of the ring in `L`.
fn tail_loop(draw: &mut Draw, text: &mut ToyText, sources: &mut Vec<ToySite>) {
    let body = any_callee(draw, text);
    let mut bodies = vec![body];
    let others = text.bodies.len() - 1;
    if others > 1 && draw.chance(1, 3) {
        bodies.push(1 + (body + draw.pick(others - 1)) % others);
    }
    let span = bodies.len();
    // Per body of the loop: its sites, the index of the site its calls sit at, and its calls,
    // each into the loop's next body.
    let (mut sites, mut firsts, mut calls) = (Vec::new(), Vec::new(), Vec::new());
    for (position, &here) in bodies.iter().enumerate() {
        let at_here: Vec<ToySite> = (std::iter::once(None))
            .chain((0..text.bodies[here].1).map(Some))
            .map(|statement| (here, statement))
            .collect();
        let first = draw.pick(at_here.len());
        let at = at_here[first];
        let callee = bodies[(position + 1) % span];
        let callers = text.entry_names(here);
        let mut out = Vec::new();
        for _ in 0..1 + usize::from(span == 1 && draw.chance(1, 4)) {
            let readable = readable(text, sources, at);
            let mut mapping = Vec::new();
            for entry in text.entry_names(callee) {
                let term = match draw.chance(3, 4) {
                    true if span == 1 => name_of(entry),
                    true => name_of(callers[draw.pick(callers.len())]),
                    false => term_over(draw, &readable),
                };
                mapping.push((entry, term));
            }
            let on = ToyOn::Site(at);
            text.edges.push(ToyEdge {
                on,
                callee,
                mapping,
            });
            out.push(text.edges.len() - 1);
        }
        sites.push(at_here);
        firsts.push(first);
        calls.push(out);
    }

    let mut kinds: Vec<Member> = (0..1 + draw.pick(3))
        .map(|_| match draw.pick(8) {
            0..=2 => Member::Takes,
            3 | 4 => Member::Alternative,
            5 => Member::Built,
            _ => Member::Decided,
        })
        .collect();
    if !kinds.contains(&Member::Takes) {
        let index = draw.pick(kinds.len());
        kinds[index] = Member::Takes;
    }
    // Across two bodies the takes come in pairs, so the ring comes back to its first body.
    let takes = kinds.iter().filter(|&&kind| kind == Member::Takes).count();
    if takes % span != 0 {
        match kinds.iter().position(|&kind| kind != Member::Takes) {
            Some(index) => kinds[index] = Member::Takes,
            None if kinds.len() == 3 => kinds[draw.pick(3)] = Member::Alternative,
            None => kinds.push(Member::Takes),
        }
    }
    let count = kinds.len();
    let start = (kinds.iter().position(|&kind| kind == Member::Takes)).expect("a taker");
    // Each name's body, as a position in `bodies`: a take moves the next name to the callee.
    let mut within = vec![0; count];
    for step in 1..count {
        let (index, before) = ((start + step) % count, (start + step - 1) % count);
        within[index] = (within[before] + usize::from(kinds[before] == Member::Takes)) % span;
    }
    // Each name's index in its body's sites, walking the ring backwards from a taker: a taker's
    // is its call's, and any other's at or after its next name's.
    let mut placed: Vec<usize> = (0..count).map(|index| firsts[within[index]]).collect();
    for back in 1..count {
        let index = (start + count - back) % count;
        if kinds[index] != Member::Takes {
            let floor = placed[(index + 1) % count];
            placed[index] = floor + draw.pick(sites[within[index]].len() - floor);
        }
    }

    let ring = text.names.len() + text.answered;
    let member = |index: usize| ring + index % count;
    let outside = sources.clone();
    for (index, &kind) in kinds.iter().enumerate() {
        let (name, next) = (member(index), member(index + 1));
        let here = within[index];
        let site = sites[here][placed[index]];
        let readable = readable(text, &outside, site);
        // The ring's takers in this body, which see this site.
        let takers: Vec<usize> = (0..count)
            .filter(|&other| kinds[other] == Member::Takes && within[other] == here)
            .map(member)
            .collect();
        let source = match kind {
            Member::Takes => {
                let edge = calls[here][draw.pick(calls[here].len())];
                ToySource::Call { edge, takes: next }
            }
            Member::Alternative | Member::Built => {
                let next = name_of(next);
                let mut alternatives = vec![if kind == Member::Built {
                    list(next)
                } else {
                    next
                }];
                for _ in 0..1 + draw.pick(2) {
                    alternatives.push(match draw.chance(1, 3) {
                        true => name_of(takers[draw.pick(takers.len())]),
                        false => alternative(draw, &readable),
                    });
                }
                let content = union(&alternatives);
                ToySource::Site { site, content }
            }
            Member::Decided => {
                let mut reads = vec![next];
                if !readable.is_empty() && draw.chance(1, 2) {
                    reads.push(readable[draw.pick(readable.len())]);
                }
                let plan = (0..1 + draw.pick(2)).map(|_| step(draw, &reads)).collect();
                let arms = 1 + draw.pick(2) as u32;
                let question = text.questions.len();
                text.questions.push(ToyQuestion {
                    site,
                    reads: reads.clone(),
                    plan,
                    arms,
                });
                let forced = draw.pick(arms as usize) as u32;
                for arm in 0..arms {
                    if arm == forced || draw.chance(1, 2) {
                        let content = term_over(draw, &reads);
                        let source = ToySource::Arm {
                            question,
                            arm,
                            content,
                        };
                        text.answers.push(ToyAnswer { name, source });
                    }
                }
                sources.push(site);
                text.answered += 1;
                continue;
            }
        };
        text.answers.push(ToyAnswer { name, source });
        sources.push(site);
        text.answered += 1;
    }

    if draw.chance(1, 2) {
        let from = any_site(draw, text);
        let mut readable = readable(text, sources, from);
        let mut on = ToyOn::Site(from);
        // Half the time on an arm of a new question, mapping only what it reads.
        if !readable.is_empty() && draw.chance(1, 2) {
            let arms = 1 + draw.pick(2) as u32;
            let asked = question_at(draw, from, &readable, arms);
            let arm = draw.pick(arms as usize) as u32;
            on = ToyOn::Arm {
                question: text.questions.len(),
                arm,
            };
            readable = asked.reads.clone();
            text.questions.push(asked);
        }
        let edge = call_into(draw, text, on, body, &readable);
        text.edges.push(edge);
        let name = text.names.len() + text.answered;
        let edge = text.edges.len() - 1;
        let in_body: Vec<usize> = (0..count).filter(|&index| within[index] == 0).collect();
        let takes = member(in_body[draw.pick(in_body.len())]);
        let source = ToySource::Call { edge, takes };
        text.answers.push(ToyAnswer { name, source });
        sources.push(from);
        text.answered += 1;
    }
}

/// A generated text, drawn in layers so it is never malformed, each layer from its own stretch
/// of `draw`. Where `closed`, every name is bound through the text: no `OpenCaller` and no
/// statement names. The root's entry binds no name, and every question reads one.
///
/// 1. Bodies and site names.
/// 2. Twice, a layer of questions, calls on arms and on sites, and answered names, each typed on a
///    nonempty set of one question's arms or through one call taking a site name of its callee's
///    entry. The second layer's reads and mentions may include a first-layer answered name
///    wherever the reader sees its source site: its question's or its call's.
/// 3. Where there is a body besides the root, one or two [`tail_loop`]s: rings of answered names
///    drawn whole, where a knot can form.
/// 4. Up to two questions with no arms, reading any name drawn, the loops' among them, and in an
///    open text, the open callers.
pub(super) fn toy_text(draw: &mut Draw, closed: bool) -> ToyText {
    let mut background = draw.split(384);
    let mut loops = draw.split(192);
    let mut late = draw.split(64);
    let draw = &mut background;
    let mut text = ToyText::default();
    let bodies = 1 + draw.pick(5);
    for index in 0..bodies {
        let encloser = draw.pick(index);
        text.bodies.push((encloser, draw.pick(4) as u32));
    }
    for body in 0..bodies {
        let entry = if body == 0 { 0 } else { 1 + draw.pick(2) };
        text.names.extend((0..entry).map(|_| (body, None)));
        for statement in 0..text.bodies[body].1 {
            let names = if closed { 0 } else { draw.pick(2) };
            text.names
                .extend((0..names).map(|_| (body, Some(statement))));
        }
    }

    // Each answered name's source site, in `NameId` order after the site names.
    let mut sources: Vec<ToySite> = Vec::new();
    for _layer in 0..2 {
        let seen = sources.clone();
        let first_question = text.questions.len();
        for _ in 0..draw.pick(5) {
            let at = any_site(draw, &text);
            let readable = readable(&text, &seen, at);
            if readable.is_empty() {
                continue;
            }
            let arms = draw.pick(3) as u32;
            let question = question_at(draw, at, &readable, arms);
            text.questions.push(question);
        }
        let with_arms: Vec<usize> = (first_question..text.questions.len())
            .filter(|&question| text.questions[question].arms > 0)
            .collect();
        let first_call = text.edges.len();
        if bodies > 1 {
            for _ in 0..draw.pick(5) {
                if with_arms.is_empty() {
                    break;
                }
                let question = with_arms[draw.pick(with_arms.len())];
                let arm = draw.pick(text.questions[question].arms as usize) as u32;
                let reads = text.questions[question].reads.clone();
                let callee = any_callee(draw, &text);
                let on = ToyOn::Arm { question, arm };
                let edge = call_into(draw, &text, on, callee, &reads);
                text.edges.push(edge);
            }
            for _ in 0..draw.pick(5) {
                let at = any_site(draw, &text);
                let readable = readable(&text, &seen, at);
                let callee = any_callee(draw, &text);
                let edge = call_into(draw, &text, ToyOn::Site(at), callee, &readable);
                text.edges.push(edge);
            }
        }
        for _ in 0..draw.pick(4) {
            let name = text.names.len() + text.answered;
            let source = if draw.chance(1, 2) {
                if with_arms.is_empty() {
                    continue;
                }
                let question = with_arms[draw.pick(with_arms.len())];
                let arms = text.questions[question].arms;
                let forced = draw.pick(arms as usize) as u32;
                let reads = text.questions[question].reads.clone();
                for arm in 0..arms {
                    if arm == forced || draw.chance(1, 2) {
                        let content = term_over(draw, &reads);
                        let source = ToySource::Arm {
                            question,
                            arm,
                            content,
                        };
                        text.answers.push(ToyAnswer { name, source });
                    }
                }
                text.questions[question].site
            } else {
                if text.edges.len() == first_call {
                    continue;
                }
                let edge = first_call + draw.pick(text.edges.len() - first_call);
                let entry = text.entry_names(text.edges[edge].callee);
                let takes = entry[draw.pick(entry.len())];
                let source = ToySource::Call { edge, takes };
                text.answers.push(ToyAnswer { name, source });
                call_site(&text, &text.edges[edge])
            };
            sources.push(source);
            text.answered += 1;
        }
    }

    if bodies > 1 {
        for _ in 0..1 + loops.pick(2) {
            tail_loop(&mut loops, &mut text, &mut sources);
        }
    }
    for _ in 0..late.pick(3) {
        let at = any_site(&mut late, &text);
        let readable = readable(&text, &sources, at);
        if !readable.is_empty() {
            let question = question_at(&mut late, at, &readable, 0);
            text.questions.push(question);
        }
    }
    if !closed {
        text.open_callers = (1..bodies).filter(|_| late.chance(1, 4)).collect();
    }
    text
}

/// The name at `index` as a term.
fn name_of(index: usize) -> Term {
    name(NameId::new(index))
}

/// A generated binding of a text with `names` names. Half are fully closed: every name bound to
/// a list over an atom. The rest leave a third of the names open and bind a tenth of the others to
/// an open name.
pub(super) fn toy_binding(draw: &mut Draw, names: usize) -> Binding<Toy> {
    let closed = draw.chance(1, 2);
    let mut binding = Binding::new();
    for index in 0..names {
        if !closed && draw.chance(1, 3) {
            continue;
        }
        let content = if !closed && draw.chance(1, 10) {
            name_of(draw.pick(names))
        } else {
            closed_term(draw)
        };
        binding.bind(NameId::new(index), content);
    }
    binding
}

/// [`toy_binding`] over `text`'s site names, with some of its answered names typed too: content
/// the evaluation takes as given.
pub(super) fn toy_binding_with_answered(draw: &mut Draw, text: &ToyText) -> Binding<Toy> {
    let mut binding = toy_binding(draw, text.names.len());
    for index in text.names.len()..text.names.len() + text.answered {
        if draw.chance(1, 4) {
            binding.bind(NameId::new(index), closed_term(draw));
        }
    }
    binding
}

/// An atom under up to two lists.
fn closed_term(draw: &mut Draw) -> Term {
    let mut term = atom(draw.pick(4) as u8);
    for _ in 0..draw.pick(3) {
        term = list(term);
    }
    term
}

/// Each name's bind site: a site name's own, and an answered name's the latest site of its base
/// in the text's order, or the root's entry where the base is empty.
pub(super) fn placed(
    text: &ToyText,
    bases: &Bases,
    position: &HashMap<ToySite, usize>,
) -> Vec<ToySite> {
    let sites = text.names.len();
    (0..sites + text.answered)
        .map(|name| {
            if name < sites {
                return text.names[name];
            }
            let base = bases.names[name].iter().map(|&site| text.names[site]);
            base.max_by_key(|at| position[at]).unwrap_or((0, None))
        })
        .collect()
}

/// A composed term cut to what growth reads: the one name it mentions, and whether a type is
/// built over it. A term mentioning no name never grows.
type Cut = (usize, bool);

/// The growth oracle, independent of the crate: composes every term along every walk by brute
/// force, matching each result to its call, and reports whether some name is ever built over
/// itself. A generated term mentions at most one name in each alternative, so each alternative
/// composes to one cut term, and a union to one per alternative.
///
/// A result taken through a call is composed from the callee's terms for the taken name over the
/// callee's **leaves**, its entry names and the names bound outside it, with the call's mapping
/// substituted for the entry names. A callee's terms compose the edges typing the names it binds,
/// each body to a fixpoint, so a result returns only to the call that asked for it.
///
/// # Panics
///
/// Past 10,000 terms, so a test fails rather than hangs.
pub(super) fn grows_by_terms(text: &ToyText) -> bool {
    let sites = text.names.len();
    let names = sites + text.answered;
    let bases = bases(text);
    let placed = placed(text, &bases, &linear_order(text, &bases));
    let within = |ancestor: usize, body: usize| ancestor == body || text.encloses(ancestor, body);
    let outside = |body: usize, name: usize| !within(body, placed[name].0);
    let entry_of = |body: usize, name: usize| name < sites && text.names[name] == (body, None);
    let mentions = |term: Term| -> Vec<Cut> {
        let mentioned = Toy.mentions(term).into_iter();
        mentioned
            .map(|(name, nesting)| (name.index(), nesting == Nesting::Built))
            .collect()
    };
    let mapped = |edge: &ToyEdge, entry: usize| {
        let pair = edge.mapping.iter().find(|(name, _)| *name == entry);
        pair.and_then(|&(_, term)| mentions(term).first().copied())
    };
    // What a term over `terms[from]` built by `built` gives `into`, through one composition.
    let compose = |into: &mut Vec<Cut>, terms: &BTreeSet<Cut>, built: bool| {
        into.extend(terms.iter().map(|&(name, deeper)| (name, built || deeper)));
    };

    // Per body, each name's terms over that body's leaves.
    let bodies = text.bodies.len();
    let mut local: Vec<Vec<BTreeSet<Cut>>> = (0..bodies)
        .map(|body| {
            (0..names)
                .map(|name| match entry_of(body, name) || outside(body, name) {
                    true => BTreeSet::from([(name, false)]),
                    false => BTreeSet::new(),
                })
                .collect()
        })
        .collect();
    loop {
        let mut changed = false;
        for body in 0..bodies {
            for answer in &text.answers {
                if outside(body, answer.name) {
                    continue;
                }
                let mut add = Vec::new();
                match &answer.source {
                    ToySource::Arm { content, .. } | ToySource::Site { content, .. } => {
                        for (from, built) in mentions(*content) {
                            compose(&mut add, &local[body][from], built);
                        }
                    }
                    ToySource::Call { edge, takes } => {
                        let call = &text.edges[*edge];
                        for &(leaf, deeper) in &local[call.callee][*takes] {
                            if entry_of(call.callee, leaf) {
                                if let Some((from, built)) = mapped(call, leaf) {
                                    compose(&mut add, &local[body][from], built || deeper);
                                }
                            } else if outside(call.callee, leaf) {
                                compose(&mut add, &local[body][leaf], deeper);
                            }
                        }
                    }
                }
                for cut in add {
                    changed |= local[body][answer.name].insert(cut);
                }
            }
        }
        if !changed {
            break;
        }
    }

    // Every walk's composition, from whichever name it starts at.
    let mut terms: Vec<BTreeSet<Cut>> = (0..names)
        .map(|name| BTreeSet::from([(name, false)]))
        .collect();
    loop {
        let mut add: Vec<(usize, Vec<Cut>)> = Vec::new();
        for edge in &text.edges {
            for &(to, term) in &edge.mapping {
                let mut cuts = Vec::new();
                for (from, built) in mentions(term) {
                    compose(&mut cuts, &terms[from], built);
                }
                add.push((to, cuts));
            }
        }
        for answer in &text.answers {
            let mut cuts = Vec::new();
            match &answer.source {
                ToySource::Arm { content, .. } | ToySource::Site { content, .. } => {
                    for (from, built) in mentions(*content) {
                        compose(&mut cuts, &terms[from], built);
                    }
                }
                ToySource::Call { edge, takes } => {
                    let call = &text.edges[*edge];
                    for &(leaf, deeper) in &local[call.callee][*takes] {
                        if entry_of(call.callee, leaf) {
                            if let Some((from, built)) = mapped(call, leaf) {
                                compose(&mut cuts, &terms[from], built || deeper);
                            }
                        } else if outside(call.callee, leaf) {
                            compose(&mut cuts, &terms[leaf], deeper);
                        }
                    }
                }
            }
            add.push((answer.name, cuts));
        }
        let mut changed = false;
        for (to, cuts) in add {
            for cut in cuts {
                if cut == (to, true) {
                    return true;
                }
                changed |= terms[to].insert(cut);
            }
        }
        let total: usize = terms.iter().map(BTreeSet::len).sum();
        assert!(total <= 10_000, "the growth oracle passed 10,000 terms");
        if !changed {
            return false;
        }
    }
}
