//! The reference evaluation the laws compare [`Bound`](crate::Bound) against. It shares no code
//! with [evaluate.rs](../bound/evaluate.rs) and runs no stack: it keeps a table of evaluations,
//! each answered name and question starting unknown, and repeats two steps until nothing is
//! unknown.
//!
//! 1. **Compute** every question whose reads are all known, and every name whose needs are all
//!    known, entering callees as the bindings they pass become known. A name whose deciding
//!    question is unknown needs only that question; one typed through a call needs the caller's
//!    names the call passes, then the callee's taken name. One typed on a site needs the names its
//!    term mentions. A question an earlier node owns is answered in the later evaluation that
//!    needs it.
//! 2. **Resolve knots.** Once nothing more is computable, take the strongly connected components,
//!    over and over, of what each unknown still needs. In each component that needs nothing
//!    outside itself, every name is its own rigid, unless every member takes or is a union
//!    alternative and the component reaches an alternative from outside: then every name is the
//!    union of those alternatives. Its questions are then computable. Each knot is recorded with
//!    its members, whether it carries one type, and whether a union typed it.
//!
//! Its components come from repeated reachability over what is still unknown, where the crate's
//! come from Tarjan's algorithm as the demands run.
//!
//! [`admission`] lists the asks a node's questions make under a full binding, with their
//! answers, reading no handle layout: the oracle for when two handles are equal.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use super::toy::{Node, Probe, Span, Term, Toy, ToyAnswers, Verdict, node};
use crate::{
    Answers, Binding, Domain, Doors, Facts, Graph, NameId, Nesting, NodeId, On, QuestionId, Source,
};

/// One evaluation the reference reached.
pub(super) struct Reached {
    pub(super) node: NodeId,
    /// The binding it was entered under, restricted to what the node can read.
    pub(super) view: Binding<Toy>,
    /// The view with every answered name of the node.
    pub(super) full: Binding<Toy>,
    /// One answer per question the node owns.
    pub(super) answers: Vec<Verdict>,
}

/// What a reference evaluation reached, and counts of what it exercised.
#[derive(Default)]
pub(super) struct Reference {
    pub(super) reached: Vec<Reached>,
    /// Names typed by an arm's content, through a call, and on a site.
    pub(super) on_arm: usize,
    pub(super) through_call: usize,
    pub(super) on_site: usize,
    /// Every knot resolved, in the order resolved.
    pub(super) knots: Vec<Knot>,
    /// Unknowns, at entry, whose name or question lies in a text component: what the crate puts
    /// on Tarjan's stack.
    pub(super) in_components: usize,
    /// Questions that asked `Which` of a rigid, asked of two names at once, and questions an
    /// earlier node owns answered later.
    pub(super) which_on_rigid: usize,
    pub(super) two_names: usize,
    pub(super) borrowed: usize,
    /// Calls on a site followed.
    pub(super) site_calls: usize,
    /// Onward calls followed, onward calls whose holder did not arrive, and names
    /// taken from a statement node onward calls reached.
    pub(super) onward: usize,
    pub(super) waited: usize,
    pub(super) reached_takes: usize,
}

/// A knot the reference resolved: its members, whether it carries one type, and whether a union
/// typed it.
pub(super) struct Knot {
    pub(super) members: Vec<Unknown>,
    /// Whether every member takes or is a union alternative.
    pub(super) one_type: bool,
    /// Whether the union of its alternatives from outside typed it, rather than each name being
    /// its own rigid.
    pub(super) typed: bool,
}

/// One unknown: an answered name's content or a question's answer, in one evaluation.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub(super) enum Unknown {
    Name(usize, NameId),
    Question(usize, QuestionId),
}

/// What an unknown still needs, and the step it takes toward it.
struct Needs {
    of: Vec<Unknown>,
    step: Step,
}

/// The step an unknown name takes toward what it still needs.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum Step {
    /// It takes the callee name its call takes, every pass known.
    Takes,
    /// It is typed on a site by a term mentioning each name it still needs only as one
    /// alternative of a union.
    Alternative,
    /// Anything else: a question, a name waiting on a pass, or a term that builds on or reads
    /// bare a name it still needs.
    Other,
}

/// Where the walk of onward calls toward a taken name's node stands.
enum Reach {
    /// An owner on the way still waits on these.
    Waits(Vec<Unknown>),
    /// Some holder on the way does not arrive: the taken name waits on the run.
    Never,
    /// The node's evaluation.
    At(usize),
}

struct Evaluation {
    node: NodeId,
    view: Binding<Toy>,
    names: BTreeMap<NameId, Option<Term>>,
    questions: BTreeMap<QuestionId, Option<Verdict>>,
    followed: bool,
}

struct Run<'g> {
    graph: &'g Graph<Toy>,
    answers: &'g mut ToyAnswers,
    evaluations: Vec<Evaluation>,
    index: HashMap<(NodeId, Binding<Toy>), usize>,
    counts: Reference,
}

/// Evaluates `node` under `binding` and every evaluation that reaches.
///
/// # Panics
///
/// Past 10,000 evaluations, so a test fails rather than hangs, or where no step can be taken.
pub(super) fn reference(
    graph: &Graph<Toy>,
    answers: &mut ToyAnswers,
    node: NodeId,
    binding: &Binding<Toy>,
) -> Reference {
    let mut run = Run {
        graph,
        answers,
        evaluations: Vec::new(),
        index: HashMap::new(),
        counts: Reference::default(),
    };
    run.enter(node, binding);
    loop {
        if run.compute() {
            continue;
        }
        let unknown = run.unknowns();
        if unknown.is_empty() {
            break;
        }
        assert!(
            run.resolve(&unknown),
            "the reference is stuck on {unknown:?}"
        );
    }
    let mut counts = run.counts;
    for evaluation in &run.evaluations {
        let owns = graph.node(evaluation.node).owns();
        let full = binding_of(evaluation);
        let answers = owns
            .iter()
            .map(|question| evaluation.questions[question].expect("known"))
            .collect();
        counts.reached.push(Reached {
            node: evaluation.node,
            view: evaluation.view.clone(),
            full,
            answers,
        });
    }
    counts
}

/// The view with every known answered name.
fn binding_of(evaluation: &Evaluation) -> Binding<Toy> {
    let mut binding = evaluation.view.clone();
    for (&name, content) in &evaluation.names {
        if let Some(content) = content {
            binding.bind(name, *content);
        }
    }
    binding
}

/// One ask an answer made: the names asked about, what was asked, and the answer.
pub(super) type Asked = (Vec<NameId>, Probe, Span);

/// Answers `question` under `binding`, recording each ask into `asked`.
fn answer_recording(
    graph: &Graph<Toy>,
    answers: &mut ToyAnswers,
    question: QuestionId,
    binding: &Binding<Toy>,
    asked: &mut Vec<Asked>,
) -> Verdict {
    let text = graph.question(question);
    let mut serve = |declared: usize| {
        let ask = &text.asks()[declared];
        let contents: Vec<_> = ask.names().iter().map(|&name| binding.get(name)).collect();
        let answer = Toy.ask(ask.question(), &contents);
        asked.push((ask.names().to_vec(), ask.question(), answer));
        answer
    };
    let mut doors = Doors::new(
        question,
        text.reads(),
        text.asks(),
        &mut serve,
        Default::default(),
    );
    answers.answer(question, &mut doors)
}

/// Every ask `node`'s questions make under `full`, which holds every name they read, with the
/// answer each took: the instance's admission.
pub(super) fn admission(
    graph: &Graph<Toy>,
    answers: &mut ToyAnswers,
    node: NodeId,
    full: &Binding<Toy>,
) -> BTreeSet<Asked> {
    let mut asked = Vec::new();
    for &question in graph.node(node).owns() {
        answer_recording(graph, answers, question, full, &mut asked);
    }
    asked.into_iter().collect()
}

impl Run<'_> {
    fn enter(&mut self, node: NodeId, binding: &Binding<Toy>) -> usize {
        let relevant = self.graph.relevant(node);
        let mut view = Binding::new();
        for (name, content) in binding.iter() {
            if relevant.contains(&name) {
                view.bind(name, content);
            }
        }
        if let Some(&at) = self.index.get(&(node, view.clone())) {
            return at;
        }
        let graph = self.graph;
        let binding_node = graph.node(node);
        let names: BTreeMap<NameId, Option<Term>> = (binding_node.answered().iter())
            .map(|&name| (name, view.get(name)))
            .collect();
        let questions: BTreeMap<QuestionId, Option<Verdict>> = (binding_node.owns().iter())
            .chain(graph.borrows(node))
            .map(|&question| (question, None))
            .collect();
        let unknown_names = names.iter().filter(|(_, content)| content.is_none());
        let names_in = unknown_names.filter(|&(&name, _)| graph.name_component(name).is_some());
        let questions_in =
            (questions.keys()).filter(|&&question| graph.question_component(question).is_some());
        self.counts.in_components += names_in.count() + questions_in.count();
        let at = self.evaluations.len();
        assert!(at < 10_000, "the reference passed 10,000 evaluations");
        self.evaluations.push(Evaluation {
            node,
            view: view.clone(),
            names,
            questions,
            followed: false,
        });
        self.index.insert((node, view), at);
        at
    }

    /// Whether `name` is known in evaluation `at`; a name the node does not bind is the view's.
    fn known(&self, at: usize, name: NameId) -> bool {
        self.evaluations[at]
            .names
            .get(&name)
            .is_none_or(Option::is_some)
    }

    /// The names a call typing a name of `at` passes that `at`'s node binds: its mentions and
    /// what its callee inherits that its site sees.
    fn passes(&self, at: usize, call: crate::EdgeId) -> Vec<NameId> {
        let mut needs = self.graph.edge(call).passes().to_vec();
        let node = self.evaluations[at].node;
        needs.retain(|&name| self.graph.answered_at(name) == Some(node));
        needs
    }

    /// One pass computing whatever is computable; whether anything was.
    fn compute(&mut self) -> bool {
        let mut progressed = false;
        let mut at = 0;
        while at < self.evaluations.len() {
            let questions: Vec<QuestionId> = (self.evaluations[at].questions.iter())
                .filter(|(_, answer)| answer.is_none())
                .map(|(&question, _)| question)
                .collect();
            for question in questions {
                let reads = self.graph.question(question).reads();
                if reads.iter().all(|&read| self.known(at, read)) {
                    let answer = self.answer(at, question);
                    self.evaluations[at]
                        .questions
                        .insert(question, Some(answer));
                    progressed = true;
                }
            }
            let names: Vec<NameId> = (self.evaluations[at].names.iter())
                .filter(|(_, content)| content.is_none())
                .map(|(&name, _)| name)
                .collect();
            for name in names {
                if let Some(content) = self.type_name(at, name) {
                    self.evaluations[at].names.insert(name, Some(content));
                    progressed = true;
                }
            }
            progressed |= self.follow(at);
            at += 1;
        }
        progressed
    }

    fn answer(&mut self, at: usize, question: QuestionId) -> Verdict {
        let evaluation = &self.evaluations[at];
        let binding = binding_of(evaluation);
        if self.graph.question(question).owner() != evaluation.node {
            self.counts.borrowed += 1;
        }
        let mut asked = Vec::new();
        let answer = answer_recording(self.graph, self.answers, question, &binding, &mut asked);
        let on_rigid = asked.iter().any(|(names, probe, _)| {
            let content = binding.get(names[0]).map(node);
            let rigid = matches!(content, Some(Node::Name(name)) if self.graph.is_answered(name));
            *probe == Probe::Which && rigid
        });
        self.counts.which_on_rigid += usize::from(on_rigid);
        self.counts.two_names += usize::from(asked.iter().any(|(names, ..)| names.len() > 1));
        answer
    }

    /// `name`'s content in `at`, where its needs are known.
    fn type_name(&mut self, at: usize, name: NameId) -> Option<Term> {
        let graph = self.graph;
        let rigid = Toy.rigid(name);
        let arm = match graph.deciding(name) {
            Some(question) => {
                let answer = self.evaluations[at].questions[&question]?;
                match Toy.arm(answer) {
                    Some(arm) => Some(arm),
                    None => return Some(rigid),
                }
            }
            None => None,
        };
        let Some(edge) = graph.typing(name, arm) else {
            return Some(rigid);
        };
        let binding = binding_of(&self.evaluations[at]);
        match graph.answers_edge(edge).source() {
            Source::Arm { content, .. } => {
                self.counts.on_arm += 1;
                Some(Toy.apply(*content, &binding).unwrap_or(rigid))
            }
            &Source::Call { edge: call, takes } => {
                if !self
                    .passes(at, call)
                    .iter()
                    .all(|&need| self.known(at, need))
                {
                    return None;
                }
                let passed = graph.passed(&Toy, call, &binding);
                let callee = graph.edge(call).callee();
                let entry = graph.entry(callee);
                let content = if graph.answered_at(takes) == Some(entry) {
                    let into = self.enter(entry, &passed);
                    self.evaluations[into].names[&takes]?
                } else if graph.reach(callee, takes).is_some() {
                    match self.reach(call, takes, &passed) {
                        Reach::Waits(_) => return None,
                        Reach::Never => rigid,
                        Reach::At(into) => {
                            let taken = self.evaluations[into].names[&takes]?;
                            self.counts.reached_takes += 1;
                            taken
                        }
                    }
                } else if graph.name_site(takes) == crate::Site::Entry(callee)
                    || !graph.inside(callee, takes)
                {
                    passed.get(takes).unwrap_or(rigid)
                } else {
                    rigid
                };
                self.counts.through_call += 1;
                Some(content)
            }
            Source::Site { content, .. } => {
                let mentions = graph.answers_edge(edge).mentions();
                if !mentions.iter().all(|&mentioned| self.known(at, mentioned)) {
                    return None;
                }
                self.counts.on_site += 1;
                Some(Toy.apply(*content, &binding).unwrap_or(rigid))
            }
        }
    }

    /// Enters each callee `at` follows, once everything of `at` is known; whether it did.
    fn follow(&mut self, at: usize) -> bool {
        let evaluation = &self.evaluations[at];
        let done = evaluation.names.values().all(Option::is_some)
            && evaluation.questions.values().all(Option::is_some);
        if evaluation.followed || !done {
            return false;
        }
        let binding = binding_of(evaluation);
        let node = evaluation.node;
        for &call in self.graph.node(node).follows() {
            let taken = match self.graph.edge(call).on() {
                On::Site(_) => {
                    self.counts.site_calls += 1;
                    true
                }
                On::Arm { question, arm } => {
                    let answer = self.evaluations[at].questions[&question].expect("known");
                    Toy.arm(answer) == Some(arm)
                }
            };
            if taken {
                let passed = self.graph.passed(&Toy, call, &binding);
                if let Some(target) = self.graph.edge(call).onward() {
                    let target = self.graph.node_at(target).expect("a node");
                    if self.arrived(target, &passed) {
                        self.counts.onward += 1;
                        self.enter(target, &passed);
                    } else {
                        self.counts.waited += 1;
                    }
                    continue;
                }
                let callee = self.graph.entry(self.graph.edge(call).callee());
                self.enter(callee, &passed);
            }
        }
        self.evaluations[at].followed = true;
        true
    }

    fn unknowns(&self) -> Vec<Unknown> {
        let mut unknown = Vec::new();
        for (at, evaluation) in self.evaluations.iter().enumerate() {
            for (&name, content) in &evaluation.names {
                if content.is_none() {
                    unknown.push(Unknown::Name(at, name));
                }
            }
            for (&question, answer) in &evaluation.questions {
                if answer.is_none() {
                    unknown.push(Unknown::Question(at, question));
                }
            }
        }
        unknown
    }

    /// The unknowns `unknown` still needs.
    fn needs(&self, unknown: Unknown) -> Needs {
        let graph = self.graph;
        let names_of = |at: usize, names: &[NameId]| -> Vec<Unknown> {
            (names.iter())
                .filter(|&&name| !self.known(at, name))
                .map(|&name| Unknown::Name(at, name))
                .collect()
        };
        let waits = |of: Vec<Unknown>| Needs {
            of,
            step: Step::Other,
        };
        match unknown {
            Unknown::Question(at, question) => {
                waits(names_of(at, graph.question(question).reads()))
            }
            Unknown::Name(at, name) => {
                let arm = match graph.deciding(name) {
                    Some(question) => match self.evaluations[at].questions[&question] {
                        None => return waits(vec![Unknown::Question(at, question)]),
                        Some(answer) => Toy.arm(answer),
                    },
                    None => None,
                };
                let edge = graph
                    .typing(name, arm)
                    .expect("an unknown name has an edge");
                if let Source::Site { content, .. } = *graph.answers_edge(edge).source() {
                    let of = names_of(at, graph.answers_edge(edge).mentions());
                    let alternative = (Toy.mentions(content).iter())
                        .filter(|(mentioned, _)| of.contains(&Unknown::Name(at, *mentioned)))
                        .all(|&(_, nesting)| nesting == Nesting::Alternative);
                    let step = if alternative {
                        Step::Alternative
                    } else {
                        Step::Other
                    };
                    return Needs { of, step };
                }
                let Source::Call { edge: call, takes } = *graph.answers_edge(edge).source() else {
                    unreachable!("a name typed on an arm is computable once its question is")
                };
                let passes = names_of(at, &self.passes(at, call));
                if !passes.is_empty() {
                    return waits(passes);
                }
                let passed = graph.passed(&Toy, call, &binding_of(&self.evaluations[at]));
                let callee = graph.edge(call).callee();
                if graph.reach(callee, takes).is_some() {
                    return match self.reached(call, takes, &passed) {
                        Reach::Waits(of) => waits(of),
                        Reach::At(into) => Needs {
                            of: vec![Unknown::Name(into, takes)],
                            step: Step::Takes,
                        },
                        Reach::Never => unreachable!("a name no chain reaches is computable"),
                    };
                }
                let entry = graph.entry(callee);
                let key = (entry, self.view_of(entry, &passed));
                let into = self.index[&key];
                Needs {
                    of: vec![Unknown::Name(into, takes)],
                    step: Step::Takes,
                }
            }
        }
    }

    /// Whether every name `target` binds and reads is closed in `given`: what a run would bind.
    fn arrived(&self, target: NodeId, given: &Binding<Toy>) -> bool {
        let relevant = self.graph.relevant(target);
        (self.graph.node(target).names().iter())
            .filter(|name| relevant.contains(name))
            .all(|&name| given.get(name).is_some_and(super::toy::closed))
    }

    /// Walks the onward calls reaching the statement node `takes` is bound at, in the callee of
    /// `call` under `passed`, entering each target as its owner's passes become known.
    fn reach(&mut self, call: crate::EdgeId, takes: NameId, passed: &Binding<Toy>) -> Reach {
        let graph = self.graph;
        let callee = graph.edge(call).callee();
        let (_, chain) = graph.reach(callee, takes).expect("a reached node");
        let mut owner = match graph.owned_inside(chain[0]) {
            true => Some(self.enter(graph.entry(callee), passed)),
            false => None,
        };
        for &onward in chain {
            let binding = match owner {
                Some(owner) => {
                    let unknown: Vec<Unknown> = (self.passes(owner, onward).iter())
                        .filter(|&&name| !self.known(owner, name))
                        .map(|&name| Unknown::Name(owner, name))
                        .collect();
                    if !unknown.is_empty() {
                        return Reach::Waits(unknown);
                    }
                    binding_of(&self.evaluations[owner])
                }
                None => passed.clone(),
            };
            let given = graph.passed(&Toy, onward, &binding);
            let target = graph
                .node_at(graph.edge(onward).onward().expect("onward"))
                .expect("a node");
            if !self.arrived(target, &given) {
                return Reach::Never;
            }
            owner = Some(self.enter(target, &given));
        }
        Reach::At(owner.expect("a chain has a call"))
    }

    /// [`Self::reach`] over the evaluations already entered.
    fn reached(&self, call: crate::EdgeId, takes: NameId, passed: &Binding<Toy>) -> Reach {
        let graph = self.graph;
        let callee = graph.edge(call).callee();
        let (_, chain) = graph.reach(callee, takes).expect("a reached node");
        let find =
            |node: NodeId, binding: &Binding<Toy>| self.index[&(node, self.view_of(node, binding))];
        let mut owner = match graph.owned_inside(chain[0]) {
            true => Some(find(graph.entry(callee), passed)),
            false => None,
        };
        for &onward in chain {
            let binding = match owner {
                Some(owner) => {
                    let unknown: Vec<Unknown> = (self.passes(owner, onward).iter())
                        .filter(|&&name| !self.known(owner, name))
                        .map(|&name| Unknown::Name(owner, name))
                        .collect();
                    if !unknown.is_empty() {
                        return Reach::Waits(unknown);
                    }
                    binding_of(&self.evaluations[owner])
                }
                None => passed.clone(),
            };
            let given = graph.passed(&Toy, onward, &binding);
            let target = graph
                .node_at(graph.edge(onward).onward().expect("onward"))
                .expect("a node");
            if !self.arrived(target, &given) {
                return Reach::Never;
            }
            owner = Some(find(target, &given));
        }
        Reach::At(owner.expect("a chain has a call"))
    }

    fn view_of(&self, node: NodeId, binding: &Binding<Toy>) -> Binding<Toy> {
        let relevant = self.graph.relevant(node);
        let mut view = Binding::new();
        for (name, content) in binding.iter() {
            if relevant.contains(&name) {
                view.bind(name, content);
            }
        }
        view
    }

    /// Resolves each knot that needs nothing outside itself: every name is the knot's one type,
    /// or its own rigid. Whether there was one.
    fn resolve(&mut self, unknown: &[Unknown]) -> bool {
        let needs: HashMap<Unknown, Needs> = (unknown.iter())
            .map(|&each| (each, self.needs(each)))
            .collect();
        // What each unknown reaches through what it needs, itself excluded unless on a cycle.
        let reach = |from: Unknown| {
            let mut seen: Vec<Unknown> = Vec::new();
            let mut pending = needs[&from].of.clone();
            while let Some(next) = pending.pop() {
                if !seen.contains(&next) {
                    seen.push(next);
                    pending.extend(needs[&next].of.iter().copied());
                }
            }
            seen
        };
        let reaches: HashMap<Unknown, Vec<Unknown>> =
            unknown.iter().map(|&each| (each, reach(each))).collect();
        let mut resolved = false;
        for &each in unknown {
            let component: Vec<Unknown> = (unknown.iter().copied())
                .filter(|&other| {
                    other == each
                        || (reaches[&each].contains(&other) && reaches[&other].contains(&each))
                })
                .collect();
            let sink = component
                .iter()
                .all(|member| needs[member].of.iter().all(|need| component.contains(need)));
            let knot = component.len() > 1 || needs[&each].of.contains(&each);
            // Each component once, from its first member.
            if !sink || !knot || component[0] != each {
                continue;
            }
            let one_type = (component.iter())
                .all(|member| matches!(needs[member].step, Step::Takes | Step::Alternative));
            let typed = if one_type {
                self.knot_type(&component, &needs)
            } else {
                None
            };
            for member in &component {
                if let Unknown::Name(at, name) = *member {
                    let content = typed.unwrap_or_else(|| Toy.rigid(name));
                    self.evaluations[at].names.insert(name, Some(content));
                }
            }
            self.counts.knots.push(Knot {
                members: component,
                one_type,
                typed: typed.is_some(),
            });
            resolved = true;
        }
        resolved
    }

    /// The one type a knot carries: the union of what each union alternative's term gives with
    /// the names it still needs as the empty union, leaving out the empty union. `None` where
    /// nothing is left, or a term cannot apply.
    fn knot_type(&self, component: &[Unknown], needs: &HashMap<Unknown, Needs>) -> Option<Term> {
        let empty = Toy.union(&[]);
        let mut outside = BTreeSet::new();
        for member in component {
            let (Unknown::Name(at, name), Step::Alternative) = (*member, needs[member].step) else {
                continue;
            };
            let edge = self.graph.typing(name, None).expect("typed on a site");
            let Source::Site { content, .. } = *self.graph.answers_edge(edge).source() else {
                unreachable!("a union alternative is typed on a site")
            };
            let mut binding = binding_of(&self.evaluations[at]);
            for need in &needs[member].of {
                let Unknown::Name(_, knotted) = *need else {
                    unreachable!("a term on a site needs only names")
                };
                binding.bind(knotted, empty);
            }
            let left = Toy.apply(content, &binding)?;
            if left != empty {
                outside.insert(left);
            }
        }
        // A term's order is its id, its digest: the crate's order.
        let alternatives: Vec<Term> = outside.into_iter().collect();
        (!alternatives.is_empty()).then(|| Toy.union(&alternatives))
    }
}
