//! Evaluation: one `BindingNode` under one view, computed demand by demand on an explicit stack,
//! within one `instance` call.
//!
//! An **evaluation** answers its node's questions, gives its answered names their content, and
//! follows the calls its node owns that its answers take, entering each callee's evaluation under
//! the binding the call passes. It ends in a handle: each declared ask its owned questions made,
//! written in its field as the answer's index plus one. A [`Session`] holds the evaluations of one
//! call, indexed by node and **view**: the digests of the binding restricted to the names the
//! node can read ([`Graph::relevant`](crate::Graph)). Content lives only here, for one call.
//!
//! An evaluation whose view the `Bound` has filed is entered **filed**: it holds its handle, is not
//! finished, and follows nothing. Where a caller takes a name from it, the name is derived again:
//! its deciding question's answer is read through the handle (from where it rests, or by replay),
//! and the arm's term is applied to the question's reads, each computed as a demand
//! ([`Phase::Apply`]). A name typed on a site applies its term to the names it mentions, as in
//! any evaluation ([`Phase::Term`]).
//!
//! A **demand** is a question's answer or an answered name's content within one evaluation. A
//! name's content comes from its deciding question's arm, from its term on a site (`Term`), or
//! through a call from a name of the callee's evaluation (`Take`), which may enter a callee before
//! the caller finishes. A question an earlier node owns that this evaluation needs is answered
//! here under its binding and written in no field ("borrowed"); no demand enters an earlier node's
//! evaluation.
//!
//! **Knots.** Demands that need each other form a strongly connected component, found by Tarjan's
//! algorithm as the demands run, never by recursion. Only a demand whose name or question lies in
//! a **text component** ([needs.rs](../graph/needs.rs)) can lie in one, so only such a demand gets
//! an index and goes on Tarjan's stack; any other reads only final demands and is final when it
//! completes. A demand read while still on Tarjan's stack lies in the reader's component: a name
//! read there reads as its own rigid, a name whose deciding question is there takes no arm, and a
//! name whose call passes a name there takes nothing from the callee. A name whose content waits
//! on a callee name there **takes** ([`Value::Takes`]), and a name typed on a site whose term
//! mentions each name there only as one alternative of a union is a **union alternative**
//! ([`Value::Alternative`]). When a component closes, a lone name that did not need itself keeps
//! what it computed. A knot whose every member takes or is a union alternative **carries one
//! type**: every name is the union of the alternatives the knot reaches from outside
//! ([`Session::knot_type`]), or its own rigid ([`Domain::rigid`]) where it reaches none. Every
//! name of any other knot is its own rigid. Under `perf`, a closed knot counts as carrying one
//! type or as another. Each need is fixed by the text and final values, so the components, and
//! every answer, are the same from every start. A filed evaluation's owned
//! question is final at once, and a name it decides reads what the question read before taking
//! an arm, and takes none where one of those lies in its knot: contracting the question out of
//! the graph of needs keeps every cycle among the names, so the knots come out the same.
//!
//! See [the crate's README](../../README.md#instances).

use std::collections::HashMap;

use super::Bound;
use super::store::{Resting, ViewKey, view_word};
use crate::domain::{Answers, Binding, Domain, Doors, Facts, NameAnswer, Nesting};
use crate::graph::{On, Source};
use crate::id::{AnswersId, EdgeId, NameId, NodeId, QuestionId, Site};

/// The evaluations of one `instance` call, and the demands under way.
pub(super) struct Session<'s, 'g, 'f, 'a, 'b, F: Facts, D: Domain<Facts = F>> {
    bound: &'s mut Bound<'g, 'f, 'a, F>,
    resting: &'s Resting<'b, F>,
    domain: &'s D,
    answers: &'s mut dyn Answers<F>,
    evaluations: Vec<Evaluation<D>>,
    /// Each evaluation by node and view.
    index: HashMap<(NodeId, ViewKey), usize>,
    /// Entered evaluations not yet finished.
    to_finish: Vec<usize>,
    /// The demands under way, innermost last. Empty between settles.
    waiting: Vec<Waiting>,
    /// Tarjan's stack: demands whose component is not yet closed, still waiting or done.
    component: Vec<Demand>,
    /// Each demand on Tarjan's stack. A demand absent here is final if its evaluation holds its
    /// value, and not yet started otherwise.
    unclosed: HashMap<Demand, Unclosed<D>>,
    next_index: usize,
}

/// One node under one view.
struct Evaluation<D: Domain> {
    node: NodeId,
    view: Binding<D>,
    key: ViewKey,
    /// The key's content word.
    word: u64,
    /// Whether every content of the view lasts.
    lasting: bool,
    /// Whether the view was filed when entered: the handle is known, and nothing is finished.
    filed: bool,
    /// Whether a filed evaluation's owned answers have been read through its handle.
    loaded: bool,
    /// Each answered name of the node, once final, parallel to its `answered`.
    names: Vec<Option<D::Content>>,
    /// Each owned question's answer, once final, parallel to `owns`.
    answers: Vec<Option<SiteAnswer<D>>>,
    /// Each borrowed question's answer, once final, parallel to the graph's `borrows`.
    borrowed: Vec<Option<SiteAnswer<D>>>,
    /// The handle, written as owned questions are answered.
    words: Vec<u64>,
}

/// A domain's `SiteAnswer`.
type SiteAnswer<D> = <<D as Domain>::Facts as Facts>::SiteAnswer;

/// One step of an evaluation: a name's content or a question's answer.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
enum Demand {
    Name(usize, NameId),
    Question(usize, QuestionId),
}

/// Where a demand under way has got to.
#[derive(Copy, Clone, Debug)]
enum Phase {
    /// A question, reading its answered names from `next` on.
    Reads { next: usize },
    /// A name, waiting on its deciding question. In a filed evaluation whose node owns the
    /// question, reading the question's reads from `next` on first.
    Decide { next: usize },
    /// A name on the arm `edge` gives, reading its question's reads from `next` on, to apply the
    /// arm's term to them.
    Apply { edge: AnswersId, next: usize },
    /// A name typed through `call`, reading the caller's names the call passes from `next` on.
    View {
        call: EdgeId,
        takes: NameId,
        next: usize,
    },
    /// A name taking `takes` from the callee's evaluation `callee`.
    Take { callee: usize, takes: NameId },
    /// A name typed on a site by `edge`'s term, reading the names it mentions from `next` on, to
    /// apply the term to them.
    Term { edge: AnswersId, next: usize },
}

/// One demand under way.
#[derive(Copy, Clone, Debug)]
struct Waiting {
    demand: Demand,
    phase: Phase,
    /// Its place in Tarjan's algorithm, where its name or question lies in a text component.
    /// A demand in none lies in no knot: it reads only final demands, and is final once done.
    books: Option<Books>,
}

/// A demand's place in Tarjan's algorithm.
#[derive(Copy, Clone, Debug)]
struct Books {
    index: usize,
    lowlink: usize,
}

/// A demand on Tarjan's stack, kept until its component closes.
struct Unclosed<D: Domain> {
    index: usize,
    /// Whether it read itself: a knot of one demand.
    needs_itself: bool,
    /// What it computed, once done.
    value: Option<Value<D>>,
}

/// What a demand computed.
enum Value<D: Domain> {
    Name(D::Content),
    /// A name whose content waits on a callee name of its own knot: it takes. Once the knot
    /// closes, it is the knot's one type, or its own rigid.
    Takes,
    /// A name typed on a site whose term mentions each name of its own knot only as one
    /// alternative of a union. Once the knot closes, it is the knot's one type, or its own rigid.
    Alternative,
    Question(SiteAnswer<D>),
}

/// Where a needed demand stands.
enum Lookup {
    Final,
    OnStack(usize),
    Fresh,
}

impl<'s, 'g, 'f, 'a, 'b, F: Facts, D: Domain<Facts = F>> Session<'s, 'g, 'f, 'a, 'b, F, D> {
    pub(super) fn new(
        bound: &'s mut Bound<'g, 'f, 'a, F>,
        resting: &'s Resting<'b, F>,
        domain: &'s D,
        answers: &'s mut dyn Answers<F>,
    ) -> Self {
        Self {
            bound,
            resting,
            domain,
            answers,
            evaluations: Vec::new(),
            index: HashMap::new(),
            to_finish: Vec::new(),
            waiting: Vec::new(),
            component: Vec::new(),
            unclosed: HashMap::new(),
            next_index: 0,
        }
    }

    /// Makes `node`'s evaluation under `binding`'s view, whose key `key` homed at `word` the
    /// caller has found unfiled, finishes every evaluation that reaches, and returns `node`'s
    /// handle words.
    pub(super) fn evaluate(
        mut self,
        node: NodeId,
        binding: &Binding<D>,
        key: ViewKey,
        word: u64,
    ) -> Vec<u64> {
        let words = vec![0; self.bound.graph.node(node).handle_width()];
        let entered = self.entered(node, binding, key, word, words, false);
        while let Some(next) = self.to_finish.pop() {
            self.finish(next);
        }
        std::mem::take(&mut self.evaluations[entered].words)
    }

    /// The evaluation of `binding`'s view of `node`: found in this session, entered filed where the
    /// `Bound` files the view, and otherwise made and queued to finish.
    fn enter(&mut self, node: NodeId, binding: &Binding<D>) -> usize {
        let probe = (node, self.bound.view_key(node, binding, self.domain));
        if let Some(&at) = self.index.get(&probe) {
            return at;
        }
        let (node, key) = probe;
        let word = view_word(node, &key);
        let mut words = vec![0; self.bound.graph.node(node).handle_width()];
        let filed = self.bound.filed(node, &key, word, &mut words);
        self.entered(node, binding, key, word, words, filed)
    }

    /// Adds the evaluation of `binding`'s view of `node`, keyed `key` and homed at `word`,
    /// holding `words`: filed, or queued to finish. Each answered name the view types is final at
    /// once.
    fn entered(
        &mut self,
        node: NodeId,
        binding: &Binding<D>,
        key: ViewKey,
        word: u64,
        words: Vec<u64>,
        filed: bool,
    ) -> usize {
        let graph = self.bound.graph;
        let relevant = graph.relevant(node);
        let view = binding.filtered(|name| relevant.binary_search(&name).is_ok());
        let binding_node = graph.node(node);
        let lasting = view.iter().all(|(_, content)| self.domain.lasts(content));
        let names = binding_node.answered().iter().map(|&name| view.get(name));
        let names = names.collect();
        let at = self.evaluations.len();
        self.evaluations.push(Evaluation {
            node,
            view,
            key: key.clone(),
            word,
            lasting,
            filed,
            loaded: false,
            names,
            answers: vec![None; binding_node.owns().len()],
            borrowed: vec![None; graph.borrows(node).len()],
            words,
        });
        self.index.insert((node, key), at);
        if !filed {
            self.to_finish.push(at);
        }
        at
    }

    /// Settles every demand of evaluation `at`, follows each call it takes, rests its answers and
    /// files its view.
    fn finish(&mut self, at: usize) {
        let graph = self.bound.graph;
        let node = self.evaluations[at].node;
        let binding_node = graph.node(node);
        for &question in binding_node.owns() {
            self.settle(Demand::Question(at, question));
        }
        for &name in binding_node.answered() {
            self.settle(Demand::Name(at, name));
        }
        for &question in graph.borrows(node) {
            self.settle(Demand::Question(at, question));
        }
        self.follow(at);

        let evaluation = &self.evaluations[at];
        let answered: Vec<SiteAnswer<D>> = (evaluation.answers.iter())
            .map(|answer| answer.expect("every owned question is settled"))
            .collect();
        self.bound
            .keep(self.resting, node, &evaluation.words, &answered);
        self.bound.file(
            node,
            &evaluation.key,
            evaluation.word,
            evaluation.lasting,
            &evaluation.words,
        );
    }

    /// Enters each callee's evaluation along the calls `at`'s node owns that its answers take.
    fn follow(&mut self, at: usize) {
        let graph = self.bound.graph;
        let node = self.evaluations[at].node;
        let binding = self.binding_of(at);
        for &call in graph.node(node).follows() {
            let taken = match graph.edge(call).on() {
                On::Site(_) => true,
                On::Arm { question, arm } => {
                    self.bound.facts.arm(self.answer_of(at, question)) == Some(arm)
                }
            };
            if taken {
                let passed = graph.passed(self.domain, call, &binding);
                self.bound.walked += 1;
                self.enter(graph.entry(graph.edge(call).callee()), &passed);
            }
        }
    }

    /// Reads a filed evaluation's owned answers through its handle.
    fn load(&mut self, at: usize) {
        let evaluation = &self.evaluations[at];
        let mut answered = Vec::new();
        self.bound.read(
            self.resting,
            evaluation.node,
            &evaluation.words,
            self.answers,
            &mut answered,
        );
        let evaluation = &mut self.evaluations[at];
        evaluation.answers = answered.into_iter().map(Some).collect();
        evaluation.loaded = true;
    }

    /// The position of answered `name` among `at`'s node's.
    fn position(&self, at: usize, name: NameId) -> usize {
        let node = self.bound.graph.node(self.evaluations[at].node);
        (node.answered().binary_search(&name)).expect("an answered name of the evaluation's node")
    }

    /// The view of evaluation `at` with every final answered name of its node.
    fn binding_of(&self, at: usize) -> Binding<D> {
        let evaluation = &self.evaluations[at];
        let answered = self.bound.graph.node(evaluation.node).answered();
        let mut binding = evaluation.view.clone();
        for (&name, content) in answered.iter().zip(&evaluation.names) {
            if let Some(content) = content {
                binding.bind(name, *content);
            }
        }
        binding
    }

    /// [`Self::binding_of`] kept to `needs`, with each that is an answered name of the node not
    /// yet final read as its own rigid: it lies in the reader's knot.
    fn reading(&self, at: usize, needs: &[NameId]) -> Binding<D> {
        let evaluation = &self.evaluations[at];
        let mut binding = Binding::new();
        for &name in needs {
            let content = if self.bound.graph.answered_at(name) == Some(evaluation.node) {
                let content = evaluation.names[self.position(at, name)];
                content.unwrap_or_else(|| self.domain.rigid(name))
            } else {
                match evaluation.view.get(name) {
                    Some(content) => content,
                    None => continue,
                }
            };
            binding.bind(name, content);
        }
        binding
    }

    /// The final answer to `question` in evaluation `at`, owned or borrowed.
    fn answer_of(&self, at: usize, question: QuestionId) -> SiteAnswer<D> {
        let evaluation = &self.evaluations[at];
        let graph = self.bound.graph;
        let answer = match graph.node(evaluation.node).owns().binary_search(&question) {
            Ok(position) => evaluation.answers[position],
            Err(_) => {
                let borrows = graph.borrows(evaluation.node);
                let position = borrows
                    .binary_search(&question)
                    .expect("a borrowed question");
                evaluation.borrowed[position]
            }
        };
        answer.expect("a question is settled before its answer is read")
    }

    fn is_final(&self, demand: Demand) -> bool {
        match demand {
            Demand::Name(at, name) => self.evaluations[at].names[self.position(at, name)].is_some(),
            Demand::Question(at, question) => {
                let evaluation = &self.evaluations[at];
                let graph = self.bound.graph;
                match graph.node(evaluation.node).owns().binary_search(&question) {
                    Ok(position) => evaluation.answers[position].is_some(),
                    Err(_) => {
                        let borrows = graph.borrows(evaluation.node);
                        let position = borrows.binary_search(&question);
                        evaluation.borrowed[position.expect("a borrowed question")].is_some()
                    }
                }
            }
        }
    }

    fn lookup(&self, demand: Demand) -> Lookup {
        if self.is_final(demand) {
            Lookup::Final
        } else if let Some(unclosed) = self.unclosed.get(&demand) {
            Lookup::OnStack(unclosed.index)
        } else {
            Lookup::Fresh
        }
    }

    /// Runs demands until `demand` is final.
    fn settle(&mut self, demand: Demand) {
        if self.is_final(demand) {
            return;
        }
        self.push(demand);
        while !self.waiting.is_empty() {
            self.step();
        }
    }

    /// Starts `demand`, fresh, on top of the stack. Only a demand whose name or question lies in
    /// a text component goes on Tarjan's stack and into `unclosed`.
    fn push(&mut self, demand: Demand) {
        let graph = self.bound.graph;
        let phase = match demand {
            Demand::Question(..) => Phase::Reads { next: 0 },
            Demand::Name(_, name) => match graph.deciding(name) {
                Some(_) => Phase::Decide { next: 0 },
                None => {
                    let edge = graph.typing(name, None).expect("an answered name is typed");
                    match *graph.answers_edge(edge).source() {
                        Source::Call { edge: call, takes } => Phase::View {
                            call,
                            takes,
                            next: 0,
                        },
                        Source::Site { .. } => Phase::Term { edge, next: 0 },
                        Source::Arm { .. } => unreachable!("an edge on no arm is on a site"),
                    }
                }
            },
        };
        let can_knot = match demand {
            Demand::Name(_, name) => graph.name_component(name).is_some(),
            Demand::Question(_, question) => graph.question_component(question).is_some(),
        };
        if !can_knot {
            debug_assert!(
                self.waiting.iter().all(|waiting| waiting.demand != demand),
                "{demand:?} reached itself, though it lies in no text component"
            );
            let books = None;
            self.waiting.push(Waiting {
                demand,
                phase,
                books,
            });
            return;
        }
        #[cfg(feature = "perf")]
        {
            self.bound.counts.stacked += 1;
        }
        let index = self.next_index;
        self.next_index += 1;
        let books = Some(Books {
            index,
            lowlink: index,
        });
        self.waiting.push(Waiting {
            demand,
            phase,
            books,
        });
        self.component.push(demand);
        let unclosed = Unclosed {
            index,
            needs_itself: false,
            value: None,
        };
        self.unclosed.insert(demand, unclosed);
    }

    /// The top demand reads one on Tarjan's stack: the two lie in one component.
    fn lower(&mut self, index: usize, read: Demand) {
        let top = self.waiting.last_mut().expect("a demand under way");
        // A demand outside every text component that read one on Tarjan's stack would lie in
        // its knot.
        let books = (top.books.as_mut())
            .expect("a demand outside every text component reads only final demands");
        books.lowlink = books.lowlink.min(index);
        if top.demand == read {
            let unclosed = self.unclosed.get_mut(&read).expect("on the stack");
            unclosed.needs_itself = true;
        }
    }

    /// Reads each of `needs`, an answered name of `at`'s node, from `next` on. Returns the
    /// position of the first not yet started, after pushing it, or `None` once all are read.
    fn read_needs(&mut self, at: usize, needs: &[NameId], next: usize) -> Option<usize> {
        let node = self.evaluations[at].node;
        for (position, &name) in needs.iter().enumerate().skip(next) {
            if self.bound.graph.answered_at(name) != Some(node) {
                continue;
            }
            let demand = Demand::Name(at, name);
            match self.lookup(demand) {
                Lookup::Final => {}
                Lookup::OnStack(index) => self.lower(index, demand),
                Lookup::Fresh => {
                    self.push(demand);
                    return Some(position);
                }
            }
        }
        None
    }

    /// Answers `question` in evaluation `at` under the reading of its reads, asking the domain
    /// each declared ask not yet answered in this answer or in a field, and writes each ask an
    /// owned question made in its field.
    fn answer(&mut self, at: usize, question: QuestionId) -> SiteAnswer<D> {
        let graph = self.bound.graph;
        let facts = self.bound.facts;
        let text = graph.question(question);
        let reading = self.reading(at, text.reads());
        let node = graph.node(self.evaluations[at].node);
        let owned = text.owner() == self.evaluations[at].node;
        let mut taken: Vec<Option<NameAnswer<D>>> = vec![None; text.asks().len()];
        #[cfg(feature = "perf")]
        let mut asked = 0;
        let domain = self.domain;
        let words = &self.evaluations[at].words;
        let mut serve = |declared: usize| {
            if let Some(answer) = taken[declared] {
                return answer;
            }
            let ask = &text.asks()[declared];
            // A field is the owner's: a borrowed question writes and reads none.
            let held = match owned {
                true => node.fields[text.fields[declared] as usize].read(words),
                false => 0,
            };
            let answer = match held {
                0 => {
                    let contents: Vec<_> = (ask.names().iter())
                        .map(|&name| reading.get(name))
                        .collect();
                    let answer = domain.ask(ask.question(), &contents);
                    let index = facts.answer_index(ask.question(), answer);
                    assert!(
                        index < facts.answer_count(ask.question()),
                        "{question:?} asked {:?}, whose answer's index {index} is past its count",
                        ask.question(),
                    );
                    #[cfg(feature = "perf")]
                    {
                        asked += 1;
                    }
                    answer
                }
                held => facts.nth_answer(ask.question(), held as u32 - 1),
            };
            taken[declared] = Some(answer);
            answer
        };
        let mut doors = Doors::new(question, text.reads(), text.asks(), &mut serve);
        let answer = self.answers.answer(question, &mut doors);
        #[cfg(feature = "perf")]
        {
            self.bound.counts.asks += asked;
            self.bound.counts.runs += 1;
        }
        if owned {
            let words = &mut self.evaluations[at].words;
            for (declared, answer) in taken.iter().enumerate() {
                if let Some(answer) = answer {
                    let question = text.asks()[declared].question();
                    let field = node.fields[text.fields[declared] as usize];
                    let index = facts.answer_index(question, *answer);
                    field.write(words, u64::from(index) + 1);
                }
            }
        }
        answer
    }

    /// Advances the top demand by one step.
    fn step(&mut self) {
        let graph = self.bound.graph;
        let facts = self.bound.facts;
        let top = *self.waiting.last().expect("a demand under way");
        let set_phase = |this: &mut Self, phase: Phase| {
            let at = this.waiting.len() - 2;
            this.waiting[at].phase = phase;
        };
        match (top.demand, top.phase) {
            (Demand::Question(at, question), Phase::Reads { next }) => {
                let reads = graph.question(question).reads();
                if let Some(position) = self.read_needs(at, reads, next) {
                    set_phase(self, Phase::Reads { next: position });
                    return;
                }
                let answer = self.answer(at, question);
                if let Some(arm) = facts.arm(answer) {
                    assert!(
                        (arm as usize) < graph.question(question).branches().len(),
                        "the answer to {question:?} took arm {arm}, which the text does not lay"
                    );
                }
                self.complete(Value::Question(answer));
            }
            (Demand::Name(at, name), Phase::Decide { next }) => {
                let question = graph.deciding(name).expect("a deciding question");
                let evaluation = &self.evaluations[at];
                let node = evaluation.node;
                if evaluation.filed && graph.question(question).owner() == node {
                    if !evaluation.loaded {
                        self.load(at);
                    }
                    // The question is final at once, so the name reads what it read, and takes
                    // no arm where one of those lies in its knot, as where the question does.
                    let reads = graph.question(question).reads();
                    if let Some(position) = self.read_needs(at, reads, next) {
                        set_phase(self, Phase::Decide { next: position });
                        return;
                    }
                    if reads.iter().any(|&read| {
                        graph.answered_at(read) == Some(node)
                            && !self.is_final(Demand::Name(at, read))
                    }) {
                        self.complete(Value::Name(self.domain.rigid(name)));
                        return;
                    }
                }
                let demand = Demand::Question(at, question);
                match self.lookup(demand) {
                    Lookup::Fresh => self.push(demand),
                    // Its deciding question lies in this name's knot: it takes no arm.
                    Lookup::OnStack(index) => {
                        self.lower(index, demand);
                        self.complete(Value::Name(self.domain.rigid(name)));
                    }
                    Lookup::Final => {
                        let arm = facts.arm(self.answer_of(at, question));
                        let typing = arm.and_then(|arm| graph.typing(name, Some(arm)));
                        let Some(edge) = typing else {
                            self.complete(Value::Name(self.domain.rigid(name)));
                            return;
                        };
                        let top = self.waiting.last_mut().expect("under way");
                        top.phase = match *graph.answers_edge(edge).source() {
                            Source::Arm { .. } => Phase::Apply { edge, next: 0 },
                            Source::Call { edge: call, takes } => Phase::View {
                                call,
                                takes,
                                next: 0,
                            },
                            Source::Site { .. } => {
                                unreachable!("a name with a deciding question is typed on its arms")
                            }
                        };
                    }
                }
            }
            (Demand::Name(at, name), Phase::Apply { edge, next }) => {
                let Source::Arm {
                    question, content, ..
                } = *graph.answers_edge(edge).source()
                else {
                    unreachable!("an applied edge sits on an arm")
                };
                let reads = graph.question(question).reads();
                if let Some(position) = self.read_needs(at, reads, next) {
                    set_phase(
                        self,
                        Phase::Apply {
                            edge,
                            next: position,
                        },
                    );
                    return;
                }
                let binding = self.reading(at, reads);
                let content = self.domain.apply(content, &binding);
                let content = content.unwrap_or_else(|| self.domain.rigid(name));
                self.complete(Value::Name(content));
            }
            (Demand::Name(at, name), Phase::Term { edge, next }) => {
                let mentions = graph.answers_edge(edge).mentions();
                if let Some(position) = self.read_needs(at, mentions, next) {
                    set_phase(
                        self,
                        Phase::Term {
                            edge,
                            next: position,
                        },
                    );
                    return;
                }
                let Source::Site { content, .. } = *graph.answers_edge(edge).source() else {
                    unreachable!("a term sits on a site")
                };
                // The names it mentions still on Tarjan's stack lie in its knot.
                let node = self.evaluations[at].node;
                let knotted: Vec<NameId> = (mentions.iter().copied())
                    .filter(|&mentioned| {
                        graph.answered_at(mentioned) == Some(node)
                            && !self.is_final(Demand::Name(at, mentioned))
                    })
                    .collect();
                if knotted.is_empty() {
                    let content = self.domain.apply(content, &self.reading(at, mentions));
                    let content = content.unwrap_or_else(|| self.domain.rigid(name));
                    self.complete(Value::Name(content));
                    return;
                }
                // A union alternative mentions each name of its knot only as one alternative.
                let alternative = (facts.mentions(content).iter())
                    .filter(|(mentioned, _)| knotted.contains(mentioned))
                    .all(|&(_, nesting)| nesting == Nesting::Alternative);
                let value = match alternative {
                    true => Value::Alternative,
                    false => Value::Name(self.domain.rigid(name)),
                };
                self.complete(value);
            }
            (Demand::Name(at, name), Phase::View { call, takes, next }) => {
                let needs = graph.edge(call).passes();
                if let Some(position) = self.read_needs(at, needs, next) {
                    let phase = Phase::View {
                        call,
                        takes,
                        next: position,
                    };
                    set_phase(self, phase);
                    return;
                }
                // A passed name still on Tarjan's stack puts this name in its knot: it takes
                // nothing, as a name whose deciding question is there takes no arm.
                let node = self.evaluations[at].node;
                if needs.iter().any(|&need| {
                    graph.answered_at(need) == Some(node) && !self.is_final(Demand::Name(at, need))
                }) {
                    self.complete(Value::Name(self.domain.rigid(name)));
                    return;
                }
                let passed = graph.passed(self.domain, call, &self.binding_of(at));
                let callee = graph.edge(call).callee();
                let entry = graph.entry(callee);
                if graph.answered_at(takes) == Some(entry) {
                    self.bound.walked += 1;
                    let callee = self.enter(entry, &passed);
                    let top = self.waiting.last_mut().expect("under way");
                    top.phase = Phase::Take { callee, takes };
                    return;
                }
                let given =
                    graph.name_site(takes) == Site::Entry(callee) || !graph.inside(callee, takes);
                let content = match passed.get(takes) {
                    Some(content) if given => content,
                    // A name the callee binds later waits on the run.
                    _ => self.domain.rigid(name),
                };
                self.complete(Value::Name(content));
            }
            (Demand::Name(..), Phase::Take { callee, takes }) => {
                let demand = Demand::Name(callee, takes);
                match self.lookup(demand) {
                    Lookup::Fresh => self.push(demand),
                    // In this name's knot: it takes, and is typed once the knot closes.
                    Lookup::OnStack(index) => {
                        self.lower(index, demand);
                        self.complete(Value::Takes);
                    }
                    Lookup::Final => {
                        let position = self.position(callee, takes);
                        let taken = self.evaluations[callee].names[position];
                        self.complete(Value::Name(taken.expect("a final name")));
                    }
                }
            }
            (demand, phase) => unreachable!("{demand:?} has no phase {phase:?}"),
        }
    }

    /// The top demand is done. One outside every text component is final at once. One on
    /// Tarjan's stack closes its component where it is the component's root: a knot that carries
    /// one type gives every name that type ([`Self::knot_type`]), and any other knot gives each its
    /// own rigid.
    fn complete(&mut self, value: Value<D>) {
        let done = self.waiting.pop().expect("a demand under way");
        let Some(books) = done.books else {
            self.finalize(done.demand, value);
            return;
        };
        let unclosed = self.unclosed.get_mut(&done.demand).expect("on the stack");
        unclosed.value = Some(value);
        if let Some(parent) = self.waiting.last_mut() {
            match &mut parent.books {
                Some(parent) => parent.lowlink = parent.lowlink.min(books.lowlink),
                // A demand in no knot pushes only demands whose components close under it.
                None => debug_assert_eq!(books.lowlink, books.index),
            }
        }
        if books.lowlink != books.index {
            return;
        }
        let root = (self.component.iter())
            .rposition(|&demand| demand == done.demand)
            .expect("the root is on Tarjan's stack");
        let members: Vec<Demand> = self.component.drain(root..).collect();
        let alone = members.len() == 1;
        let knot = !alone || self.unclosed[&members[0]].needs_itself;
        let one_type = knot
            && (members.iter()).all(|member| {
                matches!(
                    self.unclosed[member].value,
                    Some(Value::Takes | Value::Alternative)
                )
            });
        #[cfg(feature = "perf")]
        {
            match (knot, one_type) {
                (true, true) => self.bound.counts.knots_of_one_type += 1,
                (true, false) => self.bound.counts.other_knots += 1,
                (false, _) => {}
            }
        }
        let typed = if one_type {
            self.knot_type(&members)
        } else {
            None
        };
        for member in members {
            let unclosed = self.unclosed.remove(&member).expect("on the stack");
            let value = unclosed
                .value
                .expect("every member of a closed component is done");
            // A name keeps what it computed only where it closed alone and did not need itself.
            let value = match (member, value) {
                (Demand::Name(..), Value::Name(content)) if !knot => Value::Name(content),
                (Demand::Name(_, name), _) => {
                    Value::Name(typed.unwrap_or_else(|| self.domain.rigid(name)))
                }
                (_, value) => value,
            };
            self.finalize(member, value);
        }
    }

    /// The one type a knot of takes and union alternatives carries: the union of the alternatives
    /// it reaches from outside. Each union alternative's term is applied with every name of the
    /// knot it mentions bound to the empty union, and what is left, unless it is the empty union,
    /// is one alternative, kept once and in digest order. `None` where none is left, or where the
    /// domain cannot apply a term: each name is then its own rigid.
    fn knot_type(&self, members: &[Demand]) -> Option<D::Content> {
        let graph = self.bound.graph;
        let empty = self.domain.union(&[]);
        let nothing = self.domain.digest(empty);
        let mut outside: Vec<(u128, D::Content)> = Vec::new();
        for &member in members {
            let Demand::Name(at, name) = member else {
                unreachable!("a knot that carries one type holds only names")
            };
            if !matches!(self.unclosed[&member].value, Some(Value::Alternative)) {
                continue;
            }
            let edge = graph
                .typing(name, None)
                .expect("a union alternative is typed on a site");
            let Source::Site { content, .. } = *graph.answers_edge(edge).source() else {
                unreachable!("a union alternative is typed on a site")
            };
            let mentions = graph.answers_edge(edge).mentions();
            let node = self.evaluations[at].node;
            let mut binding = self.reading(at, mentions);
            for &mentioned in mentions {
                // A name of this node not yet final lies in the knot: every need outside a
                // closing component is final.
                if graph.answered_at(mentioned) == Some(node)
                    && !self.is_final(Demand::Name(at, mentioned))
                {
                    binding.bind(mentioned, empty);
                }
            }
            let left = self.domain.apply(content, &binding)?;
            let digest = self.domain.digest(left);
            if digest != nothing && outside.iter().all(|&(held, _)| held != digest) {
                outside.push((digest, left));
            }
        }
        outside.sort_unstable_by_key(|&(digest, _)| digest);
        let alternatives: Vec<D::Content> = (outside.into_iter()).map(|(_, left)| left).collect();
        (!alternatives.is_empty()).then(|| self.domain.union(&alternatives))
    }

    /// Writes `demand`'s final `value` into its evaluation.
    fn finalize(&mut self, demand: Demand, value: Value<D>) {
        let graph = self.bound.graph;
        match (demand, value) {
            (Demand::Question(at, question), Value::Question(answer)) => {
                let evaluation = &mut self.evaluations[at];
                match graph.node(evaluation.node).owns().binary_search(&question) {
                    Ok(position) => evaluation.answers[position] = Some(answer),
                    Err(_) => {
                        let borrows = graph.borrows(evaluation.node);
                        let position = borrows.binary_search(&question);
                        let position = position.expect("a borrowed question");
                        evaluation.borrowed[position] = Some(answer);
                    }
                }
            }
            (Demand::Name(at, name), Value::Name(content)) => {
                let position = self.position(at, name);
                self.evaluations[at].names[position] = Some(content);
            }
            _ => unreachable!("a final value is a name's content or a question's answer"),
        }
    }
}
