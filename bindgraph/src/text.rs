//! The description of a program's text the graph is laid from.
//!
//! The embedder names bodies and how they nest, each body's statements, the site names bound at
//! each site and the answered names that take their content from an answer, the questions at each
//! site with the names each reads, the asks its answer may make and the arms it can take, the
//! calls (`PassesName` edges on an arm or on a site, and onward calls into a statement's own
//! node), the `AnswersName` edges that type each
//! answered name (on an arm, through a call, or on a site), and the `OpenCaller` and staged marks
//! ([`Text::staged`]: a body a frame enters only where the program runs it later). Edges may
//! come in any order. Nothing here reads a binding: [`Text::lay`] turns the description into a
//! [`Graph`](crate::Graph), checking it, binding each answered name and refusing a recursion that
//! grows ([lay.rs](graph/lay.rs)).

use std::fmt;

use crate::domain::Kept;
use crate::graph::Edge;
use crate::id::{AnswersId, BodyId, EdgeId, NameId, QuestionId, Site};

/// A body as described: where it nests, how many statements it has, whether its callers are open,
/// whether it is staged.
pub(crate) struct BodyText {
    pub(crate) encloser: Option<BodyId>,
    pub(crate) statements: u32,
    pub(crate) open_caller: bool,
    pub(crate) staged: bool,
}

/// A name as described: bound at a site, or given its content by an answer.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) enum NameText {
    Site(Site),
    Answered,
}

/// A question as described.
pub(crate) struct QuestionText<K: Kept> {
    pub(crate) site: Site,
    pub(crate) reads: Vec<NameId>,
    pub(crate) arms: u32,
    /// The asks its answer may make, each over names in order, without repeats.
    pub(crate) asks: Vec<(Vec<NameId>, K::NameQuestion)>,
}

/// What selects a call, as described.
#[derive(Copy, Clone, Debug)]
pub(crate) enum OnText {
    Arm { question: QuestionId, arm: u32 },
    Site(Site),
}

/// A `PassesName` edge as described.
pub(crate) struct EdgeText<K: Kept> {
    pub(crate) on: OnText,
    /// The site whose names the call binds: a callee's entry, or an onward call's statement.
    pub(crate) target: Site,
    pub(crate) mapping: Vec<(NameId, K::Term)>,
}

impl<K: Kept> EdgeText<K> {
    /// The body the call binds names of.
    pub(crate) fn callee(&self) -> BodyId {
        self.target.body()
    }

    /// The statement an onward call binds, or `None` for a call into an entry.
    pub(crate) fn onward(&self) -> Option<Site> {
        onward(self.target)
    }
}

/// `target`, where it is a statement's site: the target of an onward call.
pub(crate) fn onward(target: Site) -> Option<Site> {
    (!matches!(target, Site::Entry(_))).then_some(target)
}

/// An `AnswersName` edge as described.
pub(crate) struct AnswersText<K: Kept> {
    pub(crate) name: NameId,
    pub(crate) source: SourceText<K>,
}

/// Where an `AnswersName` edge takes its content from, as described.
pub(crate) enum SourceText<K: Kept> {
    /// A term over the question's reads, given where its answer takes the arm.
    Arm {
        question: QuestionId,
        arm: u32,
        content: K::Term,
    },
    /// The content the callee's evaluation gives `takes`, where the call is followed.
    Call { edge: EdgeId, takes: NameId },
    /// A term over names visible at `site`, with no question deciding it.
    Site { site: Site, content: K::Term },
}

/// A builder for the description of a program's text.
pub struct Text<K: Kept> {
    pub(crate) bodies: Vec<BodyText>,
    pub(crate) names: Vec<NameText>,
    pub(crate) questions: Vec<QuestionText<K>>,
    pub(crate) edges: Vec<EdgeText<K>>,
    pub(crate) answers: Vec<AnswersText<K>>,
}

impl<K: Kept> Text<K> {
    /// A text holding only the root body, [`Text::root`].
    pub fn new() -> Self {
        let root = BodyText {
            encloser: None,
            statements: 0,
            open_caller: false,
            staged: false,
        };
        Self {
            bodies: vec![root],
            names: Vec::new(),
            questions: Vec::new(),
            edges: Vec::new(),
            answers: Vec::new(),
        }
    }

    /// The root body, whose entry is the root `BindingNode`.
    pub fn root(&self) -> BodyId {
        BodyId::new(0)
    }

    /// A body nested in `encloser`.
    pub fn body(&mut self, encloser: BodyId) -> BodyId {
        self.check_body(encloser);
        let body = BodyId::new(self.bodies.len());
        self.bodies.push(BodyText {
            encloser: Some(encloser),
            statements: 0,
            open_caller: false,
            staged: false,
        });
        body
    }

    /// Appends the next statement of `body`.
    pub fn statement(&mut self, body: BodyId) -> Site {
        self.check_body(body);
        let statements = &mut self.bodies[body.index()].statements;
        let site = Site::Statement(body, *statements);
        *statements += 1;
        site
    }

    /// A site name: one bound at `site`, whose content the binding gives.
    pub fn name(&mut self, site: Site) -> NameId {
        self.check_site(site);
        let name = NameId::new(self.names.len());
        self.names.push(NameText::Site(site));
        name
    }

    /// An answered name: one an `AnswersName` edge gives its content. Laying binds it at the
    /// `BindingNode` whose answer gives that content.
    pub fn answered_name(&mut self) -> NameId {
        let name = NameId::new(self.names.len());
        self.names.push(NameText::Answered);
        name
    }

    /// A `SiteQuestion` at `site` reading `reads`, whose answer can take one of `arms` arms.
    /// A name read twice counts once.
    pub fn question(&mut self, site: Site, reads: &[NameId], arms: u32) -> QuestionId {
        self.check_site(site);
        let mut deduplicated = Vec::with_capacity(reads.len());
        for &name in reads {
            self.check_name(name);
            if !deduplicated.contains(&name) {
                deduplicated.push(name);
            }
        }
        let question = QuestionId::new(self.questions.len());
        self.questions.push(QuestionText {
            site,
            reads: deduplicated,
            arms,
            asks: Vec::new(),
        });
        question
    }

    /// Declares that `question`'s answer may ask `asked` of `names`, in order. An ask declared
    /// twice counts once. Laying checks that each name is among the question's reads.
    pub fn may_ask(&mut self, question: QuestionId, names: &[NameId], asked: K::NameQuestion) {
        self.check_question(question);
        for &name in names {
            self.check_name(name);
        }
        let asks = &mut self.questions[question.index()].asks;
        if !asks
            .iter()
            .any(|(declared, question)| declared == names && *question == asked)
        {
            asks.push((names.to_vec(), asked));
        }
    }

    /// A call on `question`'s arm `arm` into `callee`'s entry. Each pair maps one of the callee's
    /// entry names to a term over the question's reads; a callee entry name with no pair is left
    /// open.
    pub fn passes(
        &mut self,
        question: QuestionId,
        arm: u32,
        callee: BodyId,
        mapping: Vec<(NameId, K::Term)>,
    ) -> EdgeId {
        self.check_question(question);
        self.call(OnText::Arm { question, arm }, callee, mapping)
    }

    /// A call at `site` that the text selects outright, into `callee`'s entry. Each pair maps one
    /// of the callee's entry names to a term over names visible at `site`.
    pub fn passes_at(
        &mut self,
        site: Site,
        callee: BodyId,
        mapping: Vec<(NameId, K::Term)>,
    ) -> EdgeId {
        self.check_site(site);
        self.call(OnText::Site(site), callee, mapping)
    }

    /// An **onward call** at statement `site` into that statement's own node: each pair maps one
    /// of the site's names to a term over names bound before it, the content an answer gives it.
    /// It is followed where every name it gives arrives ([`Domain::arrives`]).
    ///
    /// [`Domain::arrives`]: crate::Domain::arrives
    pub fn arrives(&mut self, site: Site, mapping: Vec<(NameId, K::Term)>) -> EdgeId {
        assert!(
            onward(site).is_some(),
            "an onward call binds a statement's names"
        );
        self.check_site(site);
        self.edge(OnText::Site(site), site, mapping)
    }

    /// An `AnswersName` edge on `question`'s arm `arm`: where the answer takes the arm, `name` gets
    /// `content`, a term over the question's reads.
    pub fn answers(
        &mut self,
        question: QuestionId,
        arm: u32,
        name: NameId,
        content: K::Term,
    ) -> AnswersId {
        self.check_question(question);
        self.check_name(name);
        let source = SourceText::Arm {
            question,
            arm,
            content,
        };
        self.answers_edge(name, source)
    }

    /// An `AnswersName` edge through `call`: where the call is followed, `name` gets the content
    /// the callee's evaluation gives `takes`, a name bound in the callee's body or visible at its
    /// entry.
    pub fn answers_through(&mut self, call: EdgeId, name: NameId, takes: NameId) -> AnswersId {
        assert!(
            call.index() < self.edges.len(),
            "{call:?} is not a call of this text"
        );
        self.check_name(name);
        self.check_name(takes);
        self.answers_edge(name, SourceText::Call { edge: call, takes })
    }

    /// An `AnswersName` edge on `site`, with no question: `name` gets `content`, a term over names
    /// visible at `site`. Laying binds `name` at the latest site in the base of what `content`
    /// mentions.
    pub fn answers_at(&mut self, site: Site, name: NameId, content: K::Term) -> AnswersId {
        self.check_site(site);
        self.check_name(name);
        self.answers_edge(name, SourceText::Site { site, content })
    }

    /// Marks `body` staged: a frame enters it only where the program runs it later, so whatever
    /// is asked or called inside it is owned no earlier than its entry.
    pub fn staged(&mut self, body: BodyId) {
        self.check_body(body);
        self.bodies[body.index()].staged = true;
    }

    /// Marks `body`'s entry `OpenCaller`: its callers are not all in the text.
    pub fn open_caller(&mut self, body: BodyId) {
        self.check_body(body);
        self.bodies[body.index()].open_caller = true;
    }

    fn call(&mut self, on: OnText, callee: BodyId, mapping: Vec<(NameId, K::Term)>) -> EdgeId {
        self.check_body(callee);
        self.edge(on, Site::Entry(callee), mapping)
    }

    fn edge(&mut self, on: OnText, target: Site, mapping: Vec<(NameId, K::Term)>) -> EdgeId {
        for (name, _) in &mapping {
            self.check_name(*name);
        }
        let edge = EdgeId::new(self.edges.len());
        self.edges.push(EdgeText {
            on,
            target,
            mapping,
        });
        edge
    }

    fn answers_edge(&mut self, name: NameId, source: SourceText<K>) -> AnswersId {
        let edge = AnswersId::new(self.answers.len());
        self.answers.push(AnswersText { name, source });
        edge
    }

    fn check_body(&self, body: BodyId) {
        assert!(
            body.index() < self.bodies.len(),
            "{body:?} is not a body of this text"
        );
    }

    fn check_name(&self, name: NameId) {
        assert!(
            name.index() < self.names.len(),
            "{name:?} is not a name of this text"
        );
    }

    fn check_question(&self, question: QuestionId) {
        assert!(
            question.index() < self.questions.len(),
            "{question:?} is not a question of this text"
        );
    }

    fn check_site(&self, site: Site) {
        self.check_body(site.body());
        if let Site::Statement(body, index) = site {
            let statements = self.bodies[body.index()].statements;
            assert!(
                index < statements,
                "{site:?} is not a statement of this text"
            );
        }
    }
}

impl<K: Kept> Default for Text<K> {
    fn default() -> Self {
        Self::new()
    }
}

/// A description the graph cannot be laid from.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Malformed {
    /// The question reads a name whose content depends on a site name bound neither on its body's
    /// lexical chain nor earlier in its own body.
    ReadOffChain { question: QuestionId, name: NameId },
    /// The edge sits on an arm its question's answer cannot take.
    ArmOutOfRange { edge: Edge },
    /// The call maps a name not bound at its target: the callee's entry, or an onward call's
    /// statement.
    MapsUnboundName { edge: EdgeId, name: NameId },
    /// The call maps one callee name twice.
    MapsNameTwice { edge: EdgeId, name: NameId },
    /// The edge on an arm, through its mapping or its content, mentions a name its question does
    /// not read.
    MentionsUnread { edge: Edge, name: NameId },
    /// A site name is bound at the root's entry, which binds none.
    RootBindsName { name: NameId },
    /// The question reads no name.
    ReadsNothing { question: QuestionId },
    /// The call or `AnswersName` edge on a site mentions a name whose content depends on a site
    /// name not visible there.
    MentionsOffChain { edge: Edge, name: NameId },
    /// The `AnswersName` edge types a site name.
    AnswersSiteName { edge: AnswersId, name: NameId },
    /// No `AnswersName` edge types the answered name.
    Unanswered { name: NameId },
    /// The edge types a name another edge types, but not on another arm of the same question.
    AnswersNameTwice { edge: AnswersId, name: NameId },
    /// The answered name's deciding question, its call's mapping or its term on a site reads the
    /// name itself, directly or through other answered names.
    AnswersOwnRead { name: NameId },
    /// The edge takes a name neither bound in the callee's body nor visible at its entry.
    TakesOffCallee { edge: AnswersId, name: NameId },
    /// The question declares an ask over no name.
    AsksNoName { question: QuestionId },
    /// The onward call's inputs hold a name its own target binds: the arrival depends on the
    /// statement it arrives at.
    ArrivesThroughItself { edge: EdgeId },
    /// A second onward call binds the statement another binds.
    ArrivesTwice { edge: EdgeId },
    /// The question declares an ask over a name it does not read.
    AsksUnread { question: QuestionId, name: NameId },
}

impl fmt::Display for Malformed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Malformed::ReadOffChain { question, name } => write!(
                f,
                "{question:?} reads {name:?}, which is not bound before it on its chain"
            ),
            Malformed::ArmOutOfRange { edge } => {
                write!(f, "{edge:?} sits on an arm its question cannot take")
            }
            Malformed::MapsUnboundName { edge, name } => {
                write!(f, "{edge:?} maps {name:?}, which its target does not bind")
            }
            Malformed::MapsNameTwice { edge, name } => write!(f, "{edge:?} maps {name:?} twice"),
            Malformed::MentionsUnread { edge, name } => write!(
                f,
                "{edge:?} mentions {name:?}, which its question does not read"
            ),
            Malformed::RootBindsName { name } => {
                write!(f, "{name:?} is bound at the root's entry, which binds none")
            }
            Malformed::ReadsNothing { question } => write!(f, "{question:?} reads no name"),
            Malformed::MentionsOffChain { edge, name } => write!(
                f,
                "{edge:?} mentions {name:?}, which is not bound before it on its chain"
            ),
            Malformed::AnswersSiteName { edge, name } => {
                write!(f, "{edge:?} types {name:?}, which is a site name")
            }
            Malformed::Unanswered { name } => write!(f, "no edge types {name:?}"),
            Malformed::AnswersNameTwice { edge, name } => write!(
                f,
                "{edge:?} types {name:?}, which another edge types off its question's other arms"
            ),
            Malformed::AnswersOwnRead { name } => {
                write!(f, "what types {name:?} reads {name:?} itself")
            }
            Malformed::TakesOffCallee { edge, name } => write!(
                f,
                "{edge:?} takes {name:?}, which is neither bound in the callee's body nor \
                 visible at its entry"
            ),
            Malformed::AsksNoName { question } => {
                write!(f, "{question:?} declares an ask over no name")
            }
            Malformed::ArrivesThroughItself { edge } => {
                write!(f, "{edge:?} reads a name the statement it arrives at binds")
            }
            Malformed::ArrivesTwice { edge } => {
                write!(f, "{edge:?} binds a statement another onward call binds")
            }
            Malformed::AsksUnread { question, name } => write!(
                f,
                "{question:?} declares an ask of {name:?}, which it does not read"
            ),
        }
    }
}

impl std::error::Error for Malformed {}
