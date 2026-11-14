//! The graph laid from a text: its bind sites in order, each question's and call's owner, where
//! each answered name binds, and the edges.
//!
//! [`Text::lay`](crate::Text::lay) ([lay.rs](graph/lay.rs)) runs once and reads no binding. It
//! computes each name's **base**, the site names its content depends on, and from the bases the
//! order of sites ([`Graph::site_le`]), the owners, and each answered name's `BindingNode`. Every
//! body's entry is a [`BindingNode`], and a statement is one only where its name is in the base
//! of a name some reader waits on. Each node but the root binds just after one other
//! ([`BindsAfter`]). Then [`grows`] refuses a closed walk of calls that builds a name over itself.
//! Last, [`needs`] finds the text components: where a knot can form under some binding.
//!
//! Laying also fixes each node's **handle** layout: one [`Field`] per distinct [`Ask`] its owned
//! questions declare, wide enough for the ask's answer indices plus a mark that it was not asked,
//! packed into whole `u64` words. A handle is an instance's admission written in those fields.
//!
//! The graph also holds the pure functions over bindings that [`Bound`](crate::Bound) and the
//! laws share: what a node sees of a binding ([`Graph::bound_at`]), what a call passes
//! ([`Graph::passed`]), and the oracle that answers a node from scratch ([`Graph::afresh`]), and
//! the facts an evaluation reads: each node's relevant names, each body's inherited names, and
//! the questions a node answers for itself though an earlier node owns them.
//!
//! See [the crate's README](../README.md#order-and-ownership).

mod grows;
mod lay;
mod needs;

use std::collections::HashMap;
use std::fmt;

use crate::domain::{Answers, Binding, Domain, Doors, Facts};
use crate::id::{AnswersId, BodyId, ComponentId, EdgeId, NameId, NodeId, QuestionId, Site};

/// One bind site in the graph: the names bound there, the `SiteQuestion`s it owns, the calls it
/// follows, and its handle's fields.
#[derive(Debug)]
pub struct BindingNode {
    site: Site,
    names: Vec<NameId>,
    answered: Vec<NameId>,
    owns: Vec<QuestionId>,
    follows: Vec<EdgeId>,
    binds_after: Option<BindsAfter>,
    open_caller: Option<OpenCaller>,
    /// One per distinct ask its owned questions declare, in declaration order.
    pub(crate) fields: Vec<Field>,
    width: usize,
}

impl BindingNode {
    /// The bind site this node is.
    pub fn site(&self) -> Site {
        self.site
    }

    /// The site names bound at the site.
    pub fn names(&self) -> &[NameId] {
        &self.names
    }

    /// The answered names bound here: those this node's answers give their content, in `NameId`
    /// order.
    pub fn answered(&self) -> &[NameId] {
        &self.answered
    }

    /// The questions this node owns, in `QuestionId` order.
    pub fn owns(&self) -> &[QuestionId] {
        &self.owns
    }

    /// The calls this node owns, in `EdgeId` order: each is followed where it is taken.
    pub fn follows(&self) -> &[EdgeId] {
        &self.follows
    }

    /// The node that binds just before this one, for every node but the root.
    pub fn binds_after(&self) -> Option<BindsAfter> {
        self.binds_after
    }

    /// The mark of an entry whose callers are not all in the text.
    pub fn open_caller(&self) -> Option<OpenCaller> {
        self.open_caller
    }

    /// The words a handle of this node takes, fixed when the text is laid.
    pub fn handle_width(&self) -> usize {
        self.width
    }
}

/// One declared ask's place in a handle: `bits` bits at `shift` in word `word`, holding 0 where
/// the ask was not made and one more than the answer's index where it was.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) struct Field {
    word: u32,
    shift: u32,
    bits: u32,
}

impl Field {
    /// The fields for answer counts `counts`, in order, packed into words without straddling one,
    /// with the words they take.
    pub(crate) fn packed(counts: impl IntoIterator<Item = u32>) -> (Vec<Field>, usize) {
        let mut fields = Vec::new();
        let (mut word, mut shift) = (0, 0);
        for count in counts {
            assert!(count > 0, "a `NameQuestion` has at least one answer");
            // Enough for 0 ("not asked") through `count` (the last index, plus one).
            let bits = u32::BITS - count.leading_zeros();
            if shift + bits > u64::BITS {
                word += 1;
                shift = 0;
            }
            fields.push(Field { word, shift, bits });
            shift += bits;
        }
        let width = if fields.is_empty() {
            0
        } else {
            word as usize + 1
        };
        (fields, width)
    }

    /// The value the field holds in `words`.
    pub(crate) fn read(self, words: &[u64]) -> u64 {
        (words[self.word as usize] >> self.shift) & self.mask()
    }

    /// Sets the field to `value` in `words`.
    pub(crate) fn write(self, words: &mut [u64], value: u64) {
        debug_assert!(value <= self.mask(), "a field's value fits its bits");
        let word = &mut words[self.word as usize];
        *word = (*word & !(self.mask() << self.shift)) | (value << self.shift);
    }

    fn mask(self) -> u64 {
        (1 << self.bits) - 1
    }
}

/// An ask a question declares its answer may make: a `NameQuestion` over one or more of its reads,
/// in order.
pub struct Ask<F: Facts> {
    names: Vec<NameId>,
    question: F::NameQuestion,
}

impl<F: Facts> Ask<F> {
    pub(crate) fn new(names: Vec<NameId>, question: F::NameQuestion) -> Self {
        Self { names, question }
    }

    /// The names asked about, in order.
    pub fn names(&self) -> &[NameId] {
        &self.names
    }

    /// What is asked of them.
    pub fn question(&self) -> F::NameQuestion {
        self.question
    }
}

impl<F: Facts> PartialEq for Ask<F> {
    fn eq(&self, other: &Self) -> bool {
        self.names == other.names && self.question == other.question
    }
}

impl<F: Facts> fmt::Debug for Ask<F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ask")
            .field("names", &self.names)
            .field("question", &self.question)
            .finish()
    }
}

/// An edge: this bind site binds just after `earlier`.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct BindsAfter {
    earlier: NodeId,
}

impl BindsAfter {
    /// The node that binds just before.
    pub fn earlier(self) -> NodeId {
        self.earlier
    }
}

/// The mark on a `BindingNode` whose callers the text cannot list. The crate holds it for the
/// embedder; no algorithm here branches on it.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct OpenCaller;

/// What the text asks at one site, with the node that owns it and the asks its answer may make.
pub struct SiteQuestion<F: Facts> {
    site: Site,
    reads: Vec<NameId>,
    owner: NodeId,
    branches: Vec<AnswerBranch>,
    asks: Vec<Ask<F>>,
    /// Per declared ask, its field among the owner's.
    pub(crate) fields: Vec<u32>,
}

impl<F: Facts> SiteQuestion<F> {
    /// The site the question sits at.
    pub fn site(&self) -> Site {
        self.site
    }

    /// Every name the question's answer depends on.
    pub fn reads(&self) -> &[NameId] {
        &self.reads
    }

    /// The node where the last site name in its reads' bases is bound.
    pub fn owner(&self) -> NodeId {
        self.owner
    }

    /// One branch per arm the answer can take, in arm order.
    pub fn branches(&self) -> &[AnswerBranch] {
        &self.branches
    }

    /// The asks the answer may make, in declaration order.
    pub fn asks(&self) -> &[Ask<F>] {
        &self.asks
    }
}

impl<F: Facts> fmt::Debug for SiteQuestion<F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SiteQuestion")
            .field("site", &self.site)
            .field("reads", &self.reads)
            .field("owner", &self.owner)
            .field("branches", &self.branches)
            .field("asks", &self.asks)
            .finish()
    }
}

/// An edge: what an answer reaches when it takes this arm.
#[derive(Debug, Default)]
pub struct AnswerBranch {
    follows: Vec<EdgeId>,
    binds: Vec<AnswersId>,
}

impl AnswerBranch {
    /// The calls on this arm.
    pub fn follows(&self) -> &[EdgeId] {
        &self.follows
    }

    /// The `AnswersName` edges on this arm, each giving one answered name a term.
    pub fn binds(&self) -> &[AnswersId] {
        &self.binds
    }
}

/// An edge that gives a name its content: a call or an `AnswersName` edge.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum Edge {
    Passes(EdgeId),
    Answers(AnswersId),
}

/// What selects a call.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum On {
    /// The call sits on an arm of a question's answer.
    Arm { question: QuestionId, arm: u32 },
    /// The text selects the call outright at this site.
    Site(Site),
}

/// An edge: a call gives its callee's entry names their content.
pub struct PassesName<F: Facts> {
    on: On,
    owner: NodeId,
    callee: BodyId,
    mapping: Vec<(NameId, F::Term)>,
    answers: Vec<AnswersId>,
    passes: Vec<NameId>,
}

impl<F: Facts> PassesName<F> {
    /// What selects this call.
    pub fn on(&self) -> On {
        self.on
    }

    /// The node that follows this call: where the last site name in its inputs' bases is bound.
    pub fn owner(&self) -> NodeId {
        self.owner
    }

    /// The body whose entry the call binds.
    pub fn callee(&self) -> BodyId {
        self.callee
    }

    /// Each mapped callee entry name, with the term over the caller's names it is given.
    pub fn mapping(&self) -> &[(NameId, F::Term)] {
        &self.mapping
    }

    /// The `AnswersName` edges through this call, each taking a callee name's content.
    pub fn answers(&self) -> &[AnswersId] {
        &self.answers
    }

    /// The caller's names the call passes, sorted: its mapping's mentions, and the names its
    /// callee inherits that its site sees.
    pub(crate) fn passes(&self) -> &[NameId] {
        &self.passes
    }
}

impl<F: Facts> fmt::Debug for PassesName<F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PassesName")
            .field("on", &self.on)
            .field("owner", &self.owner)
            .field("callee", &self.callee)
            .field("mapping", &self.mapping)
            .field("answers", &self.answers)
            .finish()
    }
}

/// An edge: an answer gives an answered name its content.
pub struct AnswersName<F: Facts> {
    name: NameId,
    source: Source<F>,
    /// For an edge on a site, the names its term mentions, sorted and without repeats: what its
    /// name waits on. Empty otherwise.
    mentions: Vec<NameId>,
}

impl<F: Facts> AnswersName<F> {
    /// The answered name this edge types.
    pub fn name(&self) -> NameId {
        self.name
    }

    /// Where the content comes from.
    pub fn source(&self) -> &Source<F> {
        &self.source
    }

    /// The names an edge on a site's term mentions, sorted: what its name waits on. Empty for an
    /// edge on an arm or through a call.
    pub(crate) fn mentions(&self) -> &[NameId] {
        &self.mentions
    }
}

impl<F: Facts> fmt::Debug for AnswersName<F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AnswersName")
            .field("name", &self.name)
            .field("source", &self.source)
            .finish()
    }
}

/// Where an `AnswersName` edge takes its content from.
pub enum Source<F: Facts> {
    /// Where `question`'s answer takes `arm`, the name gets `content`, a term over the question's
    /// reads applied to the binding.
    Arm {
        question: QuestionId,
        arm: u32,
        content: F::Term,
    },
    /// Where the call `edge` is followed, the name gets the content the callee's evaluation gives
    /// `takes`.
    Call { edge: EdgeId, takes: NameId },
    /// The name gets `content`, a term over names visible at `site` applied to the binding. No
    /// question decides it.
    Site { site: Site, content: F::Term },
}

impl<F: Facts> fmt::Debug for Source<F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Source::Arm {
                question,
                arm,
                content,
            } => f
                .debug_struct("Arm")
                .field("question", question)
                .field("arm", arm)
                .field("content", content)
                .finish(),
            Source::Call { edge, takes } => f
                .debug_struct("Call")
                .field("edge", edge)
                .field("takes", takes)
                .finish(),
            Source::Site { site, content } => f
                .debug_struct("Site")
                .field("site", site)
                .field("content", content)
                .finish(),
        }
    }
}

/// Why a text was not laid.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum LayError {
    /// The description contradicts itself.
    Malformed(crate::text::Malformed),
    /// A closed walk of calls gives `name` a type built over itself; `edge` is the edge that
    /// builds it. The name needs a declared recursive type.
    Grows { edge: Edge, name: NameId },
}

impl fmt::Display for LayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LayError::Malformed(malformed) => malformed.fmt(f),
            LayError::Grows { edge, name } => {
                write!(
                    f,
                    "a recursion through {edge:?} builds {name:?} over itself"
                )
            }
        }
    }
}

impl std::error::Error for LayError {}

impl From<crate::text::Malformed> for LayError {
    fn from(malformed: crate::text::Malformed) -> Self {
        LayError::Malformed(malformed)
    }
}

/// How the bodies nest, and the captures that order an inner entry among its encloser's
/// statements.
struct Chains {
    encloser: Vec<Option<BodyId>>,
    depth: Vec<u32>,
    /// Per body, the latest statement of its encloser in a base it or a body inside it reads.
    captured_upto: Vec<Option<u32>>,
}

impl Chains {
    /// The child of `ancestor` on the way down to `body`, where `ancestor` is a proper ancestor.
    fn child_toward(&self, ancestor: BodyId, body: BodyId) -> Option<BodyId> {
        let mut at = body;
        while let Some(up) = self.encloser[at.index()] {
            if up == ancestor {
                return Some(at);
            }
            at = up;
        }
        None
    }

    /// Whether `body` is `ancestor` or lies inside it.
    fn within(&self, ancestor: BodyId, body: BodyId) -> bool {
        ancestor == body || self.child_toward(ancestor, body).is_some()
    }

    /// Whether a name bound at `bound` is visible to a reader at `asked`: bound in a proper
    /// ancestor of its body, or earlier in the body itself.
    fn visible(&self, bound: Site, asked: Site) -> bool {
        if bound.body() == asked.body() {
            bound.rank() <= asked.rank()
        } else {
            self.child_toward(bound.body(), asked.body()).is_some()
        }
    }

    /// Whether `x` binds no later than `s` along `s`'s chain (`strict`: earlier).
    fn before(&self, x: Site, s: Site, strict: bool) -> bool {
        if x.body() == s.body() {
            return if strict {
                x.rank() < s.rank()
            } else {
                x.rank() <= s.rank()
            };
        }
        match (x, self.child_toward(x.body(), s.body())) {
            (_, None) => false,
            (Site::Entry(_), Some(_)) => true,
            (Site::Statement(_, index), Some(child)) => {
                self.captured_upto[child.index()].is_some_and(|upto| index <= upto)
            }
        }
    }

    /// The latest of `sites` along their chain: the deepest body's, latest within that body.
    fn latest(&self, sites: impl Iterator<Item = Site>) -> Option<Site> {
        sites.max_by_key(|site| (self.depth[site.body().index()], site.rank()))
    }
}

/// Where an answered name is bound and what decides it.
#[derive(Debug)]
struct Answered {
    node: NodeId,
    deciding: Option<QuestionId>,
    edges: Vec<AnswersId>,
}

/// The binding graph: a function of the text alone.
pub struct Graph<F: Facts> {
    chains: Chains,
    /// Each name's site: a site name's own, an answered name's `BindingNode`'s.
    names: Vec<Site>,
    /// Per name, where an answered name is bound; `None` for a site name.
    answered: Vec<Option<Answered>>,
    nodes: Vec<BindingNode>,
    node_at: HashMap<Site, NodeId>,
    questions: Vec<SiteQuestion<F>>,
    edges: Vec<PassesName<F>>,
    answers: Vec<AnswersName<F>>,
    /// Per node, the names its evaluation can read, sorted.
    relevant: Vec<Vec<NameId>>,
    /// Per node, the questions an earlier node owns that its evaluation answers itself, sorted.
    borrows: Vec<Vec<QuestionId>>,
    /// Per name, the text component it lies in: `None` for a site name, and for an answered name
    /// in none.
    name_components: Vec<Option<ComponentId>>,
    /// Per question, the text component it lies in.
    question_components: Vec<Option<ComponentId>>,
}

impl<F: Facts> Graph<F> {
    /// The root body's entry. It binds no site name, owns the calls with no input and the
    /// questions that need only what those calls return.
    pub fn root(&self) -> NodeId {
        NodeId::new(0)
    }

    /// Every node, in `NodeId` order: each body's entry, then the statement nodes.
    pub fn nodes(&self) -> &[BindingNode] {
        &self.nodes
    }

    /// The node `node`.
    pub fn node(&self, node: NodeId) -> &BindingNode {
        &self.nodes[node.index()]
    }

    /// The node at `site`, where the site is one.
    pub fn node_at(&self, site: Site) -> Option<NodeId> {
        self.node_at.get(&site).copied()
    }

    /// `body`'s entry node.
    pub fn entry(&self, body: BodyId) -> NodeId {
        assert!(
            body.index() < self.chains.encloser.len(),
            "{body:?} is not a body of this graph"
        );
        NodeId::new(body.index())
    }

    /// The body `body` nests in, for every body but the root.
    pub fn encloser(&self, body: BodyId) -> Option<BodyId> {
        self.chains.encloser[body.index()]
    }

    /// Every question, in `QuestionId` order.
    pub fn questions(&self) -> &[SiteQuestion<F>] {
        &self.questions
    }

    /// The question `question`.
    pub fn question(&self, question: QuestionId) -> &SiteQuestion<F> {
        &self.questions[question.index()]
    }

    /// Every call, in `EdgeId` order.
    pub fn edges(&self) -> &[PassesName<F>] {
        &self.edges
    }

    /// The call `edge`.
    pub fn edge(&self, edge: EdgeId) -> &PassesName<F> {
        &self.edges[edge.index()]
    }

    /// Every `AnswersName` edge, in `AnswersId` order.
    pub fn answers_edges(&self) -> &[AnswersName<F>] {
        &self.answers
    }

    /// The `AnswersName` edge `edge`.
    pub fn answers_edge(&self, edge: AnswersId) -> &AnswersName<F> {
        &self.answers[edge.index()]
    }

    /// Whether `name` is an answered name.
    pub fn is_answered(&self, name: NameId) -> bool {
        self.answered[name.index()].is_some()
    }

    /// The site `name` is bound at: a site name's own, an answered name's `BindingNode`'s.
    ///
    /// # Panics
    ///
    /// Where `name` is not a name of this graph's text, as in a binding made for another.
    pub fn name_site(&self, name: NameId) -> Site {
        let site = self.names.get(name.index());
        *site.unwrap_or_else(|| panic!("{name:?} is not a name of this graph"))
    }

    /// The text component `name` lies in: where a knot holding it can form under some binding.
    /// `None` for a site name, and for an answered name no knot can hold.
    pub fn name_component(&self, name: NameId) -> Option<ComponentId> {
        self.name_components[name.index()]
    }

    /// The text component `question` lies in, or `None` where no knot can hold it.
    pub fn question_component(&self, question: QuestionId) -> Option<ComponentId> {
        self.question_components[question.index()]
    }

    /// Whether `x` binds no later than `s` along `s`'s chain. An ancestor's entry binds before
    /// every site below it, and an ancestor's statement before an inner body's entry where the
    /// inner body, or one inside it, captures that statement or a later one.
    pub fn site_le(&self, x: Site, s: Site) -> bool {
        self.chains.before(x, s, false)
    }

    /// Whether `x` binds strictly before `s` along `s`'s chain.
    pub fn site_lt(&self, x: Site, s: Site) -> bool {
        self.chains.before(x, s, true)
    }

    /// What `node` sees of `binding`: the names bound at or before its site.
    pub fn bound_at<D: Domain<Facts = F>>(&self, node: NodeId, binding: &Binding<D>) -> Binding<D> {
        let site = self.node(node).site;
        binding.filtered(|name| self.site_le(self.name_site(name), site))
    }

    /// The binding `edge` passes its callee from a caller bound by `binding`: the names bound
    /// before the callee's entry, plus each mapped name given its applied content, or left open
    /// where the domain cannot compute it. The caller's own locals do not pass.
    pub fn passed<D: Domain<Facts = F>>(
        &self,
        domain: &D,
        edge: EdgeId,
        binding: &Binding<D>,
    ) -> Binding<D> {
        let edge = self.edge(edge);
        let entry = Site::Entry(edge.callee);
        let mut passed = binding.filtered(|name| self.site_lt(self.name_site(name), entry));
        for &(name, mapping) in &edge.mapping {
            if let Some(content) = domain.apply(mapping, binding) {
                passed.bind(name, content);
            }
        }
        passed
    }

    /// The oracle: `node`'s questions answered from scratch under `binding`, which holds every
    /// name they read, answered names included. Lays nothing.
    pub fn afresh<D: Domain<Facts = F>>(
        &self,
        domain: &D,
        node: NodeId,
        binding: &Binding<D>,
        answers: &mut impl Answers<F>,
    ) -> Vec<F::SiteAnswer> {
        let owns = self.node(node).owns();
        let mut answered = Vec::with_capacity(owns.len());
        for &question in owns {
            let text = self.question(question);
            let mut serve = |declared: usize| {
                let ask = &text.asks[declared];
                let contents: Vec<_> = ask.names.iter().map(|&name| binding.get(name)).collect();
                domain.ask(ask.question, &contents)
            };
            let mut doors = Doors::new(question, &text.reads, &text.asks, &mut serve);
            answered.push(answers.answer(question, &mut doors));
        }
        answered
    }

    /// The names `node`'s evaluation can read, sorted: a view keeps only these.
    pub(crate) fn relevant(&self, node: NodeId) -> &[NameId] {
        &self.relevant[node.index()]
    }

    /// The questions an earlier node owns that `node`'s evaluation answers itself: deciding
    /// questions of its answered names, and selecting questions of its calls.
    pub(crate) fn borrows(&self, node: NodeId) -> &[QuestionId] {
        &self.borrows[node.index()]
    }

    /// Whether `name` is bound in `body` or a body inside it.
    pub(crate) fn inside(&self, body: BodyId, name: NameId) -> bool {
        self.chains.within(body, self.name_site(name).body())
    }

    /// The node an answered name is bound at, or `None` for a site name.
    pub(crate) fn answered_at(&self, name: NameId) -> Option<NodeId> {
        self.answered[name.index()].as_ref().map(|at| at.node)
    }

    /// The question an answered name's edges hang on, if any.
    pub(crate) fn deciding(&self, name: NameId) -> Option<QuestionId> {
        self.answered[name.index()].as_ref()?.deciding
    }

    /// The edge typing answered `name` where its deciding question's answer takes `arm`, or its
    /// one edge on a site, through a call or a term, where `arm` is `None`.
    pub(crate) fn typing(&self, name: NameId, arm: Option<u32>) -> Option<AnswersId> {
        let at = self.answered[name.index()].as_ref()?;
        at.edges.iter().copied().find(|&edge| {
            let on_arm = match self.answers_edge(edge).source {
                Source::Arm { arm, .. } => Some(arm),
                Source::Call { edge, .. } => match self.edge(edge).on {
                    On::Arm { arm, .. } => Some(arm),
                    On::Site(_) => None,
                },
                Source::Site { .. } => None,
            };
            on_arm == arm
        })
    }
}
