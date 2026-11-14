//! Hand-written cases of handles, the frozen part, budgets, view tables and replays: a handle's
//! fields, a name derived again from a filed callee, a replay that asks nothing, the bound on what
//! a `Bound` keeps, and what holding no content means for the types.

use std::cell::Cell;

use bumpstorage::Bump;

use super::cases::{Case, bind};
use super::harness::{Counting, EMPTY, Kept, handle, read};
use super::reference::reference;
use super::toy::{Probe, Span, Step, Term, Toy, ToyAnswers, Verdict, atom, list, name, pair};
use crate::{
    Answers, Binding, Bound, Domain, Doors, Frozen, Graph, Handle, LayError, Malformed, NameId,
    NodeId, QuestionId, Resting, Site,
};

const NUMBER: u8 = 0;
const STR: u8 = 1;

/// The toy, counting the asks it answers.
struct CountingToy<'c> {
    asks: &'c Cell<usize>,
}

impl Domain for CountingToy<'_> {
    type Facts = Toy;
    type Content = Term;

    fn ask(&self, question: Probe, contents: &[Option<Term>]) -> Span {
        self.asks.set(self.asks.get() + 1);
        Toy.ask(question, contents)
    }

    fn apply(&self, term: Term, binding: &Binding<Self>) -> Option<Term> {
        Toy.apply(term, &toy_binding(binding.iter()))
    }

    fn union(&self, alternatives: &[Term]) -> Term {
        Toy.union(alternatives)
    }

    fn rigid(&self, name: NameId) -> Term {
        Toy.rigid(name)
    }

    fn digest(&self, content: Term) -> u128 {
        Toy.digest(content)
    }

    fn lasts(&self, content: Term) -> bool {
        Toy.lasts(content)
    }
}

/// The toy binding of `pairs`.
fn toy_binding(pairs: impl Iterator<Item = (NameId, Term)>) -> Binding<Toy> {
    let mut binding = Binding::new();
    for (name, content) in pairs {
        binding.bind(name, content);
    }
    binding
}

/// The answers the oracle gives `node` under the full binding the reference reaches from
/// `binding`.
fn expected(
    graph: &Graph<Toy>,
    answers: &mut ToyAnswers,
    node: NodeId,
    binding: &Binding<Toy>,
) -> Vec<Verdict> {
    let full = reference(graph, answers, node, binding)
        .reached
        .swap_remove(0)
        .full;
    graph.afresh(&Toy, node, &full, answers)
}

/// A body with entry names `x` and `y`, and a question at its entry probing `x` for its leaf and
/// its depth.
fn probed() -> (Graph<Toy>, ToyAnswers, NodeId, NameId, NameId) {
    let mut case = Case::new();
    let body = case.text.body(case.text.root());
    let x = case.text.name(Site::Entry(body));
    let y = case.text.name(Site::Entry(body));
    let plan = [(x, Probe::Leaf), (x, Probe::Depth)];
    case.ask(Site::Entry(body), &[x], &plan, 0);
    let (graph, answers) = case.lay();
    let entry = graph.entry(body);
    (graph, answers, entry, x, y)
}

#[test]
fn a_malformed_ask_is_refused() {
    let mut case = Case::new();
    let body = case.text.body(case.text.root());
    let x = case.text.name(Site::Entry(body));
    let y = case.text.name(Site::Entry(body));
    let question = case.ask(Site::Entry(body), &[x], &[], 0);
    case.text.may_ask(question, &[x, y], Probe::Same);
    assert_eq!(
        case.refusal(),
        LayError::Malformed(Malformed::AsksUnread { question, name: y })
    );

    let mut case = Case::new();
    let body = case.text.body(case.text.root());
    let x = case.text.name(Site::Entry(body));
    let question = case.ask(Site::Entry(body), &[x], &[], 0);
    case.text.may_ask(question, &[], Probe::Depth);
    assert_eq!(
        case.refusal(),
        LayError::Malformed(Malformed::AsksNoName { question })
    );
}

#[test]
#[should_panic(expected = "does not declare")]
fn an_undeclared_ask_panics() {
    let mut case = Case::new();
    let body = case.text.body(case.text.root());
    let x = case.text.name(Site::Entry(body));
    case.text.question(Site::Entry(body), &[x], 0);
    let step = Step {
        names: vec![x],
        probe: Probe::Depth,
        guarded: false,
    };
    case.answers.plans.push((vec![step], 0));
    let (graph, mut answers) = case.lay();
    graph.afresh(&Toy, graph.entry(body), &Binding::new(), &mut answers);
}

/// Two questions of one node declaring one ask share its field; a node with no ask takes no word;
/// four asks of 17 bits take two words, since a field never straddles one.
#[test]
fn each_declared_ask_takes_one_field() {
    let mut case = Case::new();
    let root = case.text.root();
    let shared = case.text.body(root);
    let x = case.text.name(Site::Entry(shared));
    let first = case.ask(Site::Entry(shared), &[x], &[(x, Probe::Depth)], 0);
    let plan = [(x, Probe::Leaf), (x, Probe::Depth)];
    let second = case.ask(Site::Entry(shared), &[x], &plan, 0);
    let silent = case.text.body(root);
    let s = case.text.name(Site::Entry(silent));
    case.ask(Site::Entry(silent), &[s], &[], 0);
    let wide = case.text.body(root);
    let w = case.text.name(Site::Entry(wide));
    let v = case.text.name(Site::Entry(wide));
    let plan = [
        (w, Probe::Depth),
        (w, Probe::Leaf),
        (v, Probe::Depth),
        (v, Probe::Leaf),
    ];
    case.ask(Site::Entry(wide), &[w, v], &plan, 0);
    let (graph, _) = case.lay();

    let node = graph.node(graph.entry(shared));
    assert_eq!(node.fields.len(), 2);
    assert_eq!(node.handle_width(), 1);
    assert_eq!(graph.question(first).fields, [0]);
    assert_eq!(graph.question(second).fields, [1, 0]);
    assert_eq!(graph.node(graph.entry(silent)).handle_width(), 0);
    assert_eq!(graph.node(graph.entry(wide)).handle_width(), 2);
}

/// The answered-name program in the toy:
///
/// ```text
/// EXPR #(TAG n :Number) -> Number = #(n)
/// EXPR #(TAG s :Str) -> Str = #(s)
/// EXPR #(SHOW n :Number) -> Str = #("number")
/// EXPR #(SHOW s :Str) -> Str = #("string")
/// EXPR FOR ALL #[Elem] #(REPORT x :Elem) -> Str = #(
///   LET tagged = (TAG x)
///   SHOW tagged
/// )
/// PRINT (REPORT 5)
/// PRINT (REPORT "hi")
/// ```
///
/// `REPORT`'s result is typed by `SHOW`'s arm over `tagged`, which `TAG`'s arm types. With
/// `REPORT` under `Elem := Number` filed first, a caller taking its result derives it again: the
/// arms read through `REPORT`'s handle, the terms applied, and no answer function of `REPORT`'s
/// run.
#[test]
fn a_taken_name_is_derived_from_a_filed_callee() {
    let mut case = Case::new();
    let root = case.text.root();
    let report = case.text.body(root);
    let x = case.text.name(Site::Entry(report));
    let tagged = case.text.answered_name();
    let result = case.text.answered_name();
    let tag = case.ask(Site::Entry(report), &[x], &[(x, Probe::Leaf)], 2);
    case.text.answers(tag, 0, tagged, atom(NUMBER));
    case.text.answers(tag, 1, tagged, atom(STR));
    let show = case.ask(Site::Entry(report), &[tagged], &[(tagged, Probe::Leaf)], 2);
    case.text.answers(show, 0, result, list(name(tagged)));
    case.text.answers(show, 1, result, list(list(name(tagged))));
    let printed = [NUMBER, STR].map(|elem| {
        let statement = case.text.statement(root);
        let call = case.call(statement, report, vec![(x, atom(elem))]);
        let printed = case.text.answered_name();
        case.text.answers_through(call, printed, result);
        (statement, printed)
    });
    let shown = case.text.statement(root);
    let probes = printed.map(|(_, printed)| (printed, Probe::Depth));
    let reads = printed.map(|(_, printed)| printed);
    case.ask(shown, &reads, &probes, 0);
    let (graph, mut answers) = case.lay();

    let bump = Bump::new();
    let mut kept = Kept::new(&graph, &bump);
    let entry = graph.entry(report);
    kept.instance(entry, &bind(&[(x, atom(NUMBER))]), &mut answers);
    let mut counting = Counting::new(&mut answers);
    let read = kept.answers(graph.root(), &Binding::new(), &mut counting);
    let report_runs = (counting.runs.iter())
        .filter(|question| [tag, show].contains(question))
        .count();
    // `REPORT "hi"` is new: its two questions run once each. `REPORT 5` runs none.
    assert_eq!(report_runs, 2);
    assert_eq!(
        read,
        expected(&graph, &mut answers, graph.root(), &Binding::new())
    );
    assert_eq!(read[0].span, Span::exact(1 + 2));
}

/// With a budget of 0 nothing rests, so every read replays: the answer function runs, and the
/// domain is asked nothing.
#[test]
fn a_replay_asks_nothing() {
    let (graph, mut answers, entry, x, _) = probed();
    let lend = Bump::new();
    let mut bound = Bound::new(&graph, &Toy, &EMPTY, &lend, None);
    let run = Bump::new();
    let resting = Resting::new(&run, Some(0));
    let asks = Cell::new(0);
    let domain = CountingToy { asks: &asks };
    let binding = bind(&[(x, list(atom(3)))]);
    let mut counted = Binding::new();
    counted.bind(x, list(atom(3)));
    let mut words = vec![0; graph.node(entry).handle_width()];
    let mut counting = Counting::new(&mut answers);
    bound.instance(resting, entry, &counted, &domain, &mut counting, &mut words);
    assert_eq!(asks.get(), 2);
    assert!(resting.passed() && resting.is_empty());
    let mut read = Vec::new();
    bound.answers(
        resting,
        Handle::new(entry, &words),
        &mut counting,
        &mut read,
    );
    assert_eq!(asks.get(), 2);
    assert_eq!(counting.runs.len(), 2);
    assert_eq!(read, graph.afresh(&Toy, entry, &binding, &mut answers));
}

/// An answer function that probes the leaf on its first run and the depth after.
struct Fickle {
    name: NameId,
    runs: usize,
}

impl Answers<Toy> for Fickle {
    fn answer(&mut self, _: QuestionId, doors: &mut Doors<'_, Toy>) -> Verdict {
        let probe = if self.runs == 0 {
            Probe::Leaf
        } else {
            Probe::Depth
        };
        self.runs += 1;
        Verdict {
            span: doors.ask(&[self.name], probe),
            arm: None,
        }
    }
}

#[test]
#[should_panic(expected = "not deterministic")]
fn a_nondeterministic_answer_function_panics_on_replay() {
    let (graph, _, entry, x, _) = probed();
    let lend = Bump::new();
    let mut bound = Bound::new(&graph, &Toy, &EMPTY, &lend, None);
    let run = Bump::new();
    let resting = Resting::new(&run, Some(0));
    let mut fickle = Fickle { name: x, runs: 0 };
    let words = handle(
        &mut bound,
        resting,
        entry,
        &bind(&[(x, atom(1))]),
        &mut fickle,
    );
    read(&mut bound, resting, entry, &words, &mut fickle);
}

/// The acceptance test: a body shown new content at every turn of a loop, none of it lasting,
/// over 40 instances. Its views thrash a table of two slots and its answers pass a budget of
/// 1,024 bytes, the store dropped and its bump reset each time. Neither bump ever grows, and every
/// handle reads as the oracle.
#[test]
fn a_view_of_new_content_each_turn_stays_bounded() {
    let (graph, mut answers, entry, x, y) = probed();
    let lend = Bump::new();
    let mut bound = Bound::new(&graph, &Toy, &EMPTY, &lend, Some(2));
    let mut run = Bump::with_capacity(1024);
    let (mut run_size, mut lent_size) = (None, None);
    let mut resets = 0;
    let mut turn = 0;
    while turn < 1000 {
        let resting = Resting::new(&run, Some(1024));
        let run_size = *run_size.get_or_insert(run.allocated_bytes());
        while turn < 1000 {
            let mut content = pair(atom((turn % 256) as u8), name(y));
            for _ in 0..turn % 40 {
                content = list(content);
            }
            let binding = bind(&[(x, content)]);
            let words = handle(&mut bound, resting, entry, &binding, &mut answers);
            let read = read(&mut bound, resting, entry, &words, &mut answers);
            assert_eq!(read, graph.afresh(&Toy, entry, &binding, &mut answers));
            assert_eq!(run.allocated_bytes(), run_size, "turn {turn}");
            let lent_size = *lent_size.get_or_insert(lend.allocated_bytes());
            assert_eq!(lend.allocated_bytes(), lent_size, "turn {turn}");
            turn += 1;
            if resting.passed() {
                break;
            }
        }
        run.reset();
        resets += 1;
    }
    assert!(resets > 1, "the budget is passed");
}

#[test]
fn nothing_kept_carries_drop_glue() {
    const {
        assert!(!std::mem::needs_drop::<Bound<'static, 'static, 'static, Toy>>());
        assert!(!std::mem::needs_drop::<Resting<'static, Toy>>());
        assert!(!std::mem::needs_drop::<Frozen<'static, Toy>>());
        assert!(!std::mem::needs_drop::<Handle<'static>>());
    }
}

#[test]
fn the_frozen_part_is_sync() {
    fn sync<T: Sync>() {}
    sync::<Frozen<'static, Toy>>();
}

/// A scope a content borrows: what a step's type chain is to koan.
#[derive(PartialEq, Eq, Hash, Debug)]
struct Scope(u8);

/// The toy over contents that borrow a `Scope` local to one call.
struct Scoped<'c> {
    scope: &'c Scope,
}

impl<'c> Domain for Scoped<'c> {
    type Facts = Toy;
    type Content = (Term, &'c Scope);

    fn ask(&self, question: Probe, contents: &[Option<(Term, &'c Scope)>]) -> Span {
        let terms: Vec<Option<Term>> = (contents.iter())
            .map(|content| content.map(|(term, _)| term))
            .collect();
        Toy.ask(question, &terms)
    }

    fn apply(&self, term: Term, binding: &Binding<Self>) -> Option<(Term, &'c Scope)> {
        let terms = toy_binding(binding.iter().map(|(name, (term, _))| (name, term)));
        Some((Toy.apply(term, &terms)?, self.scope))
    }

    fn union(&self, alternatives: &[(Term, &'c Scope)]) -> (Term, &'c Scope) {
        let terms: Vec<Term> = alternatives.iter().map(|&(term, _)| term).collect();
        (Toy.union(&terms), self.scope)
    }

    fn rigid(&self, name: NameId) -> (Term, &'c Scope) {
        (Toy.rigid(name), self.scope)
    }

    fn digest(&self, (term, _): (Term, &'c Scope)) -> u128 {
        Toy.digest(term)
    }

    fn lasts(&self, (term, _): (Term, &'c Scope)) -> bool {
        Toy.lasts(term)
    }
}

/// A content may borrow a local of its call: the `Bound` outlives the scope, and the instance it
/// made under it is the toy's.
#[test]
fn a_content_may_borrow_its_call() {
    let (graph, mut answers, entry, x, _) = probed();
    let lend = Bump::new();
    let mut bound = Bound::new(&graph, &Toy, &EMPTY, &lend, None);
    let run = Bump::new();
    let resting = Resting::new(&run, None);
    let mut words = vec![0; graph.node(entry).handle_width()];
    {
        let scope = Scope(7);
        let mut binding = Binding::new();
        binding.bind(x, (list(atom(3)), &scope));
        let domain = Scoped { scope: &scope };
        bound.instance(resting, entry, &binding, &domain, &mut answers, &mut words);
    }
    let bump = Bump::new();
    let mut kept = Kept::new(&graph, &bump);
    let toy = kept.instance(entry, &bind(&[(x, list(atom(3)))]), &mut answers);
    assert_eq!(words, toy);
    assert_eq!(
        read(&mut bound, resting, entry, &words, &mut answers),
        kept.read(entry, &toy, &mut answers)
    );
}
