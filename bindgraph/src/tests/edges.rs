//! Hand-written cases of the calls on sites and the `AnswersName` edges: owners and placement,
//! each way a name takes a callee's, growth through results, the knots, their rigids and the tail
//! loops a union types, the demands each knot case puts on Tarjan's stack, and the evaluations
//! kept by view.
//!
//! The design's examples are written in the toy: `Number` is `Atom(0)` and `Str` is `Atom(1)`,
//! a dispatch is a question probing a leaf, and a candidate's return is an arm's content. A case
//! reads a name's content through a question that probes it, with `Probe::Which` telling rigids
//! apart.

use bumpstorage::Bump;

use super::cases::{Case, bind};
use super::harness::Kept;
use super::toy::{Probe, Span, Toy, ToyAnswers, Verdict, atom, list, name, peel, union};
use crate::{
    Answers, Binding, BodyId, Doors, Edge, EdgeId, Graph, LayError, NameId, QuestionId, Site,
};

const NUMBER: u8 = 0;
const STR: u8 = 1;

/// The span `question`'s owner answered for it in the instance filed under `binding`'s view.
fn answered(
    kept: &mut Kept<'_, '_>,
    question: QuestionId,
    binding: &Binding<Toy>,
    answers: &mut ToyAnswers,
) -> Span {
    let graph = kept.bound.graph();
    let owner = graph.question(question).owner();
    let position = graph.node(owner).owns().binary_search(&question);
    let found = kept.found(owner, binding, answers).expect("an instance");
    found[position.expect("owned")].span
}

/// What `Probe::Which` answers of `name`'s own rigid.
fn which(name: NameId) -> Span {
    Span::exact(name.index() as u32)
}

/// A question at `site` probing `name` with `probe`, deciding nothing.
fn probe(case: &mut Case, site: Site, name: NameId, probe: Probe) -> QuestionId {
    case.ask(site, &[name], &[(name, probe)], 0)
}

#[test]
fn a_call_on_a_site_is_owned_by_its_latest_mention() {
    let mut case = Case::new();
    let root = case.text.root();
    let callee = case.text.body(root);
    let c = case.text.name(Site::Entry(callee));
    let body = case.text.body(root);
    let x = case.text.name(Site::Entry(body));
    let first = case.text.statement(body);
    let u = case.text.name(first);
    let second = case.text.statement(body);
    let by_statement = case.call(second, callee, vec![(c, name(u))]);
    let by_entry = case.call(second, callee, vec![(c, name(x))]);
    let top = case.text.statement(root);
    let by_root = case.call(top, callee, vec![(c, atom(0))]);
    // In an inner body, a call mentioning the encloser's statement name captures it.
    let inner = case.text.body(body);
    let by_capture = case.call(Site::Entry(inner), callee, vec![(c, list(name(u)))]);
    let (graph, _) = case.lay();
    let first = graph.node_at(first).expect("a node");
    assert_eq!(graph.edge(by_statement).owner(), first);
    assert_eq!(graph.edge(by_entry).owner(), graph.entry(body));
    assert_eq!(graph.edge(by_root).owner(), graph.root());
    assert_eq!(graph.edge(by_capture).owner(), first);
    let after = graph.node(graph.entry(inner)).binds_after();
    assert_eq!(after.map(|edge| edge.earlier()), Some(first));
    assert_eq!(graph.node(first).follows(), &[by_statement, by_capture]);
}

/// `OUTER`/`INNER 3`: `INNER` reads `kind`, whose base is `OUTER`'s `Elem`, so the call `INNER 3`
/// counts `Elem` among its inputs. `OUTER`'s entry binds `kind`, owns the call, and passes `kind`.
#[test]
fn a_call_counts_its_callees_captures() {
    let mut case = Case::new();
    let root = case.text.root();
    let outer = case.text.body(root);
    let x = case.text.name(Site::Entry(outer));
    let inner = case.text.body(outer);
    let y = case.text.name(Site::Entry(inner));
    let kind = case.text.answered_name();
    let kind_of = case.ask(Site::Entry(inner), &[x], &[(x, Probe::Leaf)], 2);
    case.text.answers(kind_of, 0, kind, atom(NUMBER));
    case.text.answers(kind_of, 1, kind, atom(STR));
    let joined = case.text.statement(inner);
    let join = case.ask(joined, &[kind, y], &[(kind, Probe::Leaf)], 2);
    let at = case.text.statement(outer);
    let call = case.call(at, inner, vec![(y, atom(NUMBER))]);
    let top = case.text.statement(root);
    case.call(top, outer, vec![(x, atom(STR))]);
    let (graph, mut answers) = case.lay();
    assert_eq!(graph.node(graph.entry(outer)).answered(), &[kind]);
    assert_eq!(graph.question(kind_of).owner(), graph.entry(outer));
    assert_eq!(graph.edge(call).owner(), graph.entry(outer));
    assert_eq!(graph.question(join).owner(), graph.entry(inner));

    let bound_bump = Bump::new();
    let mut bound = Kept::new(&graph, &bound_bump);
    bound.instance(graph.root(), &Binding::new(), &mut answers);
    let inner_binding = bind(&[(y, atom(NUMBER)), (kind, atom(STR))]);
    assert_eq!(
        answered(&mut bound, join, &inner_binding, &mut answers),
        Span::exact(u32::from(STR))
    );
}

/// An arm call whose callee captures a run-typed name bound after its question's owner is owned
/// by that name's `BindingNode`, which answers the question itself.
#[test]
fn a_call_owned_later_answers_its_question_itself() {
    let mut case = Case::new();
    let root = case.text.root();
    let body = case.text.body(root);
    let x = case.text.name(Site::Entry(body));
    let first = case.text.statement(body);
    let u = case.text.name(first);
    let inner = case.text.body(body);
    let y = case.text.name(Site::Entry(inner));
    let probed = case.ask(Site::Entry(inner), &[u, y], &[(u, Probe::Leaf)], 0);
    let second = case.text.statement(body);
    let select = case.ask(second, &[x], &[(x, Probe::Leaf)], 2);
    let call = case.text.passes(select, 1, inner, vec![(y, name(x))]);
    let (graph, mut answers) = case.lay();
    let first = graph.node_at(first).expect("a node");
    assert_eq!(graph.question(select).owner(), graph.entry(body));
    assert_eq!(graph.edge(call).owner(), first);
    assert_eq!(graph.borrows(first), &[select]);

    let bound_bump = Bump::new();
    let mut bound = Kept::new(&graph, &bound_bump);
    bound.instance(first, &bind(&[(x, atom(1)), (u, atom(3))]), &mut answers);
    let inner_binding = bind(&[(u, atom(3)), (y, atom(1))]);
    assert_eq!(
        answered(&mut bound, probed, &inner_binding, &mut answers),
        Span::exact(3)
    );
    // Where the answer takes the other arm, nothing is followed.
    let walked = bound.bound.walked();
    bound.instance(first, &bind(&[(x, atom(0)), (u, atom(3))]), &mut answers);
    assert_eq!(bound.bound.walked(), walked);
}

/// An answer function that records, for each question it answers, the call that entered the
/// instance answering it.
struct Entered<'a> {
    inner: &'a mut ToyAnswers,
    seen: Vec<(QuestionId, Option<EdgeId>)>,
}

impl Answers<Toy> for Entered<'_> {
    fn answer(&mut self, question: QuestionId, doors: &mut Doors<'_, Toy>) -> Verdict {
        self.seen.push((question, doors.entered()));
        self.inner.answer(question, doors)
    }
}

#[test]
fn an_answer_sees_the_call_that_entered_its_instance() {
    let mut case = Case::new();
    let root = case.text.root();
    let callee = case.text.body(root);
    let c = case.text.name(Site::Entry(callee));
    let probed = probe(&mut case, Site::Entry(callee), c, Probe::Leaf);
    let first = case.text.statement(root);
    let by_number = case.call(first, callee, vec![(c, atom(NUMBER))]);
    let second = case.text.statement(root);
    let by_str = case.call(second, callee, vec![(c, atom(STR))]);
    let again = case.text.statement(root);
    case.call(again, callee, vec![(c, atom(NUMBER))]);
    let (graph, mut answers) = case.lay();

    let bump = Bump::new();
    let mut kept = Kept::new(&graph, &bump);
    let mut entered = Entered {
        inner: &mut answers,
        seen: Vec::new(),
    };
    kept.instance(graph.root(), &Binding::new(), &mut entered);
    // Each view is answered once, under the first call that entered it.
    let mut seen: Vec<_> = (entered.seen.iter())
        .filter(|(question, _)| *question == probed)
        .map(|&(_, edge)| edge)
        .collect();
    seen.sort();
    assert_eq!(seen, [Some(by_number), Some(by_str)]);
}

/// `PAIR`: one `BindingNode`, its entry, binds `a` and `b` and owns all three dispatches. Under
/// `Elem := Number`, `JOIN`'s answer is the `Number` candidate's. After the load, `PAIR`'s view is
/// found by its view, and evaluating it again walks and makes nothing.
#[test]
fn pair_binds_its_lets_at_its_entry() {
    let mut case = Case::new();
    let root = case.text.root();
    let pair = case.text.body(root);
    let x = case.text.name(Site::Entry(pair));
    let y = case.text.name(Site::Entry(pair));
    let a = case.text.answered_name();
    let b = case.text.answered_name();
    let show = |case: &mut Case, read: NameId, typed: NameId| {
        let at = case.text.statement(pair);
        let show = case.ask(at, &[read], &[(read, Probe::Leaf)], 2);
        case.text.answers(show, 0, typed, atom(NUMBER));
        case.text.answers(show, 1, typed, atom(STR));
        show
    };
    let show_x = show(&mut case, x, a);
    let show_y = show(&mut case, y, b);
    let joined = case.text.statement(pair);
    let join = case.ask(joined, &[a, b], &[(a, Probe::Leaf)], 2);
    let top = case.text.statement(root);
    case.call(top, pair, vec![(x, atom(NUMBER)), (y, atom(NUMBER))]);
    let (graph, mut answers) = case.lay();
    let entry = graph.entry(pair);
    let in_pair = graph
        .nodes()
        .iter()
        .filter(|node| node.site().body() == pair);
    assert_eq!(in_pair.count(), 1);
    assert_eq!(graph.node(entry).answered(), &[a, b]);
    assert_eq!(graph.node(entry).owns(), &[show_x, show_y, join]);

    let bound_bump = Bump::new();
    let mut bound = Kept::new(&graph, &bound_bump);
    bound.instance(graph.root(), &Binding::new(), &mut answers);
    let view = bind(&[(x, atom(NUMBER)), (y, atom(NUMBER))]);
    let found = bound.found(entry, &view, &mut answers).expect("the load's");
    assert_eq!(found[2].arm, Some(0));
    let filed = bound.bound.filed_view(entry, &view, &Toy);
    let (walked, laid) = (bound.bound.walked(), bound.resting.laid());
    assert_eq!(Some(bound.instance(entry, &view, &mut answers)), filed);
    assert_eq!((bound.bound.walked(), bound.resting.laid()), (walked, laid));
}

/// `PACK`'s body dispatches `BOX x`, whose candidate's return types `PACK`'s result.
fn pack(case: &mut Case) -> (BodyId, NameId, NameId) {
    let pack = case.text.body(case.text.root());
    let x = case.text.name(Site::Entry(pack));
    let result = case.text.answered_name();
    let boxed = case.ask(Site::Entry(pack), &[x], &[(x, Probe::Leaf)], 2);
    case.text.answers(boxed, 0, result, list(atom(NUMBER)));
    case.text.answers(boxed, 1, result, list(atom(STR)));
    (pack, x, result)
}

/// `packed` and `relayed` are typed after the load, over one hop and over two.
#[test]
fn pack_and_relay_type_their_lets() {
    let mut case = Case::new();
    let root = case.text.root();
    let (pack, pack_x, pack_result) = pack(&mut case);
    let relay = case.text.body(root);
    let relay_x = case.text.name(Site::Entry(relay));
    let relay_result = case.text.answered_name();
    let call = case.call(Site::Entry(relay), pack, vec![(pack_x, name(relay_x))]);
    case.text.answers_through(call, relay_result, pack_result);
    let first = case.text.statement(root);
    let call = case.call(first, pack, vec![(pack_x, atom(NUMBER))]);
    let packed = case.text.answered_name();
    case.text.answers_through(call, packed, pack_result);
    let second = case.text.statement(root);
    let call = case.call(second, relay, vec![(relay_x, atom(STR))]);
    let relayed = case.text.answered_name();
    case.text.answers_through(call, relayed, relay_result);
    let third = case.text.statement(root);
    let show_packed = probe(&mut case, third, packed, Probe::Leaf);
    let show_relayed = probe(&mut case, third, relayed, Probe::Leaf);
    let (graph, mut answers) = case.lay();
    assert_eq!(graph.node(graph.root()).answered(), &[packed, relayed]);
    assert_eq!(graph.node(graph.entry(relay)).answered(), &[relay_result]);

    let bound_bump = Bump::new();
    let mut bound = Kept::new(&graph, &bound_bump);
    bound.instance(graph.root(), &Binding::new(), &mut answers);
    let none = Binding::new();
    assert_eq!(
        answered(&mut bound, show_packed, &none, &mut answers),
        Span::exact(u32::from(NUMBER))
    );
    assert_eq!(
        answered(&mut bound, show_relayed, &none, &mut answers),
        Span::exact(u32::from(STR))
    );
}

/// The inherited takes: `WRAP`'s result has an empty base, so the root binds it, and a caller of
/// `WRAP` takes it from the binding the call passes. `INNER2`'s result is bound at its encloser.
#[test]
fn a_result_bound_outside_its_body_is_taken() {
    let mut case = Case::new();
    let root = case.text.root();
    let (pack, pack_x, pack_result) = pack(&mut case);
    let wrap = case.text.body(root);
    let n = case.text.name(Site::Entry(wrap));
    let wrap_result = case.text.answered_name();
    let call = case.call(Site::Entry(wrap), pack, vec![(pack_x, atom(NUMBER))]);
    case.text.answers_through(call, wrap_result, pack_result);
    let top = case.text.statement(root);
    let call = case.call(top, wrap, vec![(n, atom(0))]);
    let wrapped = case.text.answered_name();
    case.text.answers_through(call, wrapped, wrap_result);
    let shown = case.text.statement(root);
    let show_wrapped = probe(&mut case, shown, wrapped, Probe::Leaf);

    let outer = case.text.body(root);
    let x = case.text.name(Site::Entry(outer));
    let inner = case.text.body(outer);
    let y = case.text.name(Site::Entry(inner));
    let inner_result = case.text.answered_name();
    let decide = case.ask(Site::Entry(inner), &[x], &[(x, Probe::Leaf)], 2);
    case.text.answers(decide, 0, inner_result, atom(5));
    case.text.answers(decide, 1, inner_result, atom(6));
    let at = case.text.statement(outer);
    let call = case.call(at, inner, vec![(y, atom(0))]);
    let taken = case.text.answered_name();
    case.text.answers_through(call, taken, inner_result);
    let show_taken = probe(&mut case, at, taken, Probe::Leaf);
    let top = case.text.statement(root);
    case.call(top, outer, vec![(x, atom(1))]);

    let (graph, mut answers) = case.lay();
    assert_eq!(graph.node(graph.root()).answered(), &[wrap_result, wrapped]);
    let outer_entry = graph.node(graph.entry(outer));
    assert_eq!(outer_entry.answered(), &[inner_result, taken]);

    let bound_bump = Bump::new();
    let mut bound = Kept::new(&graph, &bound_bump);
    bound.instance(graph.root(), &Binding::new(), &mut answers);
    let wrapped_type = answered(&mut bound, show_wrapped, &Binding::new(), &mut answers);
    assert_eq!(wrapped_type, Span::exact(u32::from(NUMBER)));
    assert_eq!(
        answered(&mut bound, show_taken, &bind(&[(x, atom(1))]), &mut answers),
        Span::exact(6)
    );
}

/// What a name takes from the callee: an unread site name of its entry, from the binding the call
/// passes; a result bound at a run-typed statement, which waits on the run, gives its rigid.
#[test]
fn each_take_reads_its_callee() {
    let mut case = Case::new();
    let root = case.text.root();
    let callee = case.text.body(root);
    let c = case.text.name(Site::Entry(callee));
    let statement = case.text.statement(callee);
    let u = case.text.name(statement);
    let result = case.text.answered_name();
    let decide = case.ask(statement, &[u], &[(u, Probe::Leaf)], 1);
    case.text.answers(decide, 0, result, name(u));
    let top = case.text.statement(root);
    let call = case.call(top, callee, vec![(c, atom(3))]);
    let from_entry = case.text.answered_name();
    let from_statement = case.text.answered_name();
    case.text.answers_through(call, from_entry, c);
    case.text.answers_through(call, from_statement, result);
    let shown = case.text.statement(root);
    let show_entry = probe(&mut case, shown, from_entry, Probe::Leaf);
    let show_statement = probe(&mut case, shown, from_statement, Probe::Which);
    let (graph, mut answers) = case.lay();
    assert_eq!(graph.answered_at(result), graph.node_at(statement));

    let bound_bump = Bump::new();
    let mut bound = Kept::new(&graph, &bound_bump);
    bound.instance(graph.root(), &Binding::new(), &mut answers);
    let none = Binding::new();
    assert_eq!(
        answered(&mut bound, show_entry, &none, &mut answers),
        Span::exact(3)
    );
    assert_eq!(
        answered(&mut bound, show_statement, &none, &mut answers),
        which(from_statement)
    );
}

/// A body that wraps its entry name into a result, takes the result through one call and calls
/// itself on it through a second builds the name over itself through a result.
#[test]
fn growth_through_a_result_is_refused() {
    let mut case = Case::new();
    let body = case.text.body(case.text.root());
    let x = case.text.name(Site::Entry(body));
    let result = case.text.answered_name();
    let wraps = case.ask(Site::Entry(body), &[x], &[(x, Probe::Depth)], 1);
    case.text.answers(wraps, 0, result, list(name(x)));
    let first = case.text.statement(body);
    let call = case.call(first, body, vec![(x, name(x))]);
    let rest = case.text.answered_name();
    let takes = case.text.answers_through(call, rest, result);
    let second = case.text.statement(body);
    case.call(second, body, vec![(x, name(rest))]);
    assert_eq!(
        case.refusal(),
        LayError::Grows {
            edge: Edge::Answers(takes),
            name: rest
        }
    );
}

/// `C1` calls `ID` with `List(a)`, and `C2` calls `ID` with `b`, then `C1` with the result. Each
/// result returns to its own call, so nothing grows.
#[test]
fn a_result_returns_only_to_its_call() {
    let mut case = Case::new();
    let root = case.text.root();
    let id = case.text.body(root);
    let e = case.text.name(Site::Entry(id));
    let c1 = case.text.body(root);
    let a = case.text.name(Site::Entry(c1));
    let call = case.call(Site::Entry(c1), id, vec![(e, list(name(a)))]);
    let c1_result = case.text.answered_name();
    case.text.answers_through(call, c1_result, e);
    let c2 = case.text.body(root);
    let b = case.text.name(Site::Entry(c2));
    let first = case.text.statement(c2);
    let call = case.call(first, id, vec![(e, name(b))]);
    let identity = case.text.answered_name();
    case.text.answers_through(call, identity, e);
    let second = case.text.statement(c2);
    let call = case.call(second, c1, vec![(a, name(identity))]);
    let c2_result = case.text.answered_name();
    case.text.answers_through(call, c2_result, c1_result);
    case.lay();
}

/// A body `L` with entry name `x`, a self-call `L x` at its first statement, and a root call
/// `L 0`: the frame the knot cases build on.
struct Looped {
    case: Case,
    body: BodyId,
    x: NameId,
    call: EdgeId,
}

fn looped() -> Looped {
    let mut case = Case::new();
    let root = case.text.root();
    let body = case.text.body(root);
    let x = case.text.name(Site::Entry(body));
    let first = case.text.statement(body);
    let call = case.call(first, body, vec![(x, name(x))]);
    let top = case.text.statement(root);
    case.call(top, body, vec![(x, atom(NUMBER))]);
    Looped {
        case,
        body,
        x,
        call,
    }
}

/// `BOX x` inside `L`: `boxed` is typed on the two arms of a dispatch reading `x`, the
/// `Number` candidate's module or the `Str` candidate's.
fn boxed(case: &mut Case, body: BodyId, x: NameId) -> NameId {
    let boxed = case.text.answered_name();
    let dispatch = case.ask(Site::Entry(body), &[x], &[(x, Probe::Leaf)], 2);
    case.text.answers(dispatch, 0, boxed, list(atom(NUMBER)));
    case.text.answers(dispatch, 1, boxed, list(atom(STR)));
    boxed
}

/// What one `instance` call on a fresh `Bound` puts on Tarjan's stack and closes, counted
/// under `perf`: the demands whose name or question lies in a text component, and the knots,
/// carrying one type and of any other kind.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[cfg_attr(not(feature = "perf"), allow(dead_code))]
struct Stacked {
    demands: u64,
    knots_of_one_type: u64,
    other_knots: u64,
}

/// Each probe's span at `L`'s view, after evaluating from the root, and in a fresh `Bound` from
/// `L`'s entry. Both starts agree. Under `perf`, each start, one `instance` call on a fresh
/// `Bound`, counts `stacked`.
fn from_each_start(
    graph: &Graph<Toy>,
    answers: &mut ToyAnswers,
    body: BodyId,
    x: NameId,
    probes: &[QuestionId],
    stacked: Stacked,
) -> Vec<Span> {
    let view = bind(&[(x, atom(NUMBER))]);
    let root_bump = Bump::new();
    let mut from_root = Kept::new(graph, &root_bump);
    from_root.instance(graph.root(), &Binding::new(), answers);
    let body_bump = Bump::new();
    let mut from_body = Kept::new(graph, &body_bump);
    from_body.instance(graph.entry(body), &view, answers);
    #[cfg(feature = "perf")]
    for kept in [&from_root, &from_body] {
        let counts = kept.bound.counts();
        let counted = Stacked {
            demands: counts.stacked,
            knots_of_one_type: counts.knots_of_one_type,
            other_knots: counts.other_knots,
        };
        assert_eq!(counted, stacked);
    }
    #[cfg(not(feature = "perf"))]
    let _ = stacked;
    let mut spans = |kept: &mut Kept<'_, '_>| -> Vec<Span> {
        probes
            .iter()
            .map(|&question| answered(kept, question, &view, answers))
            .collect()
    };
    let spans_from_root = spans(&mut from_root);
    assert_eq!(spans_from_root, spans(&mut from_body));
    spans_from_root
}

/// `LOOP`: `rest` takes the result through the self-call, and `BOX x` types the result without
/// needing `rest`. So there is no knot, and `rest` is `BOX`'s module.
#[test]
fn loop_takes_its_own_result() {
    let Looped {
        mut case,
        body,
        x,
        call,
    } = looped();
    let result = case.text.answered_name();
    let rest = case.text.answered_name();
    case.text.answers_through(call, rest, result);
    let boxed = case.ask(Site::Entry(body), &[x], &[(x, Probe::Leaf)], 2);
    case.text.answers(boxed, 0, result, list(atom(NUMBER)));
    case.text.answers(boxed, 1, result, list(atom(STR)));
    let shown = probe(&mut case, Site::Entry(body), rest, Probe::Leaf);
    let (graph, mut answers) = case.lay();
    let spans = from_each_start(
        &graph,
        &mut answers,
        body,
        x,
        &[shown],
        Stacked {
            demands: 0,
            knots_of_one_type: 0,
            other_knots: 0,
        },
    );
    assert_eq!(spans, vec![Span::exact(u32::from(NUMBER))]);
}

/// `LOOP` ending in `SHOW rest`: the result hangs on `SHOW`'s dispatch, which reads `rest`, which
/// takes the result. Each is its own rigid, from either start and in both declaration orders.
fn loop_shows_rest(
    rest_first: bool,
    shows: Probe,
) -> (
    Graph<Toy>,
    ToyAnswers,
    BodyId,
    NameId,
    NameId,
    NameId,
    Vec<QuestionId>,
) {
    let Looped {
        mut case,
        body,
        x,
        call,
    } = looped();
    let (result, rest) = if rest_first {
        let rest = case.text.answered_name();
        (case.text.answered_name(), rest)
    } else {
        (case.text.answered_name(), case.text.answered_name())
    };
    case.text.answers_through(call, rest, result);
    let show = case.ask(Site::Entry(body), &[rest], &[(rest, shows)], 1);
    case.text.answers(show, 0, result, name(rest));
    // The caller's view of the result.
    let root = case.text.root();
    let top = case.text.statement(root);
    let call = case.call(top, body, vec![(x, atom(NUMBER))]);
    let taken = case.text.answered_name();
    case.text.answers_through(call, taken, result);
    let show_taken = probe(&mut case, top, taken, Probe::Which);
    let (graph, answers) = case.lay();
    (
        graph,
        answers,
        body,
        x,
        result,
        rest,
        vec![show, show_taken],
    )
}

#[test]
fn loop_showing_rest_knots_into_two_rigids() {
    for rest_first in [false, true] {
        let (graph, mut answers, body, x, result, rest, probes) =
            loop_shows_rest(rest_first, Probe::Which);
        let spans = from_each_start(
            &graph,
            &mut answers,
            body,
            x,
            &probes[..1],
            Stacked {
                demands: 3,
                knots_of_one_type: 0,
                other_knots: 1,
            },
        );
        assert_eq!(spans, vec![which(rest)]);
        let bound_bump = Bump::new();
        let mut bound = Kept::new(&graph, &bound_bump);
        bound.instance(graph.root(), &Binding::new(), &mut answers);
        assert_eq!(
            answered(&mut bound, probes[1], &Binding::new(), &mut answers),
            which(result)
        );
    }
}

/// The run's type for a name the load made rigid makes a new instance, filed under its view.
#[test]
fn a_binding_that_types_an_answered_name_is_taken_as_given() {
    let (graph, mut answers, body, x, _, rest, probes) = loop_shows_rest(false, Probe::Which);
    let bound_bump = Bump::new();
    let mut bound = Kept::new(&graph, &bound_bump);
    bound.instance(graph.root(), &Binding::new(), &mut answers);
    let loaded = bound.resting();
    let entry = graph.entry(body);
    let run = bind(&[(x, atom(NUMBER)), (rest, list(atom(0)))]);
    let made = bound.instance(entry, &run, &mut answers);
    assert_eq!(bound.resting(), loaded + 1);
    assert_eq!(bound.bound.filed_view(entry, &run, &Toy), Some(made));
    let unbounded = Span {
        lo: 0,
        hi: Span::MAX,
    };
    assert_eq!(
        answered(&mut bound, probes[0], &run, &mut answers),
        unbounded
    );
}

/// The permutations of `0..3`.
const ORDERS: [[usize; 3]; 6] = [
    [0, 1, 2],
    [0, 2, 1],
    [1, 0, 2],
    [1, 2, 0],
    [2, 0, 1],
    [2, 1, 0],
];

/// `L` with `E`, `S` and `T`, each taking the next through the self-call: a knot made only of
/// takes. All three are rigid, in any declaration order.
#[test]
fn a_knot_of_takes_gives_each_its_own_rigid() {
    for order in ORDERS {
        let Looped {
            mut case,
            body,
            x,
            call,
        } = looped();
        let mut names = [NameId::new(0); 3];
        for slot in order {
            names[slot] = case.text.answered_name();
        }
        for (index, &taker) in names.iter().enumerate() {
            case.text
                .answers_through(call, taker, names[(index + 1) % 3]);
        }
        let probes: Vec<QuestionId> = names
            .iter()
            .map(|&name| probe(&mut case, Site::Entry(body), name, Probe::Which))
            .collect();
        let (graph, mut answers) = case.lay();
        let spans = from_each_start(
            &graph,
            &mut answers,
            body,
            x,
            &probes,
            Stacked {
                demands: 3,
                knots_of_one_type: 1,
                other_knots: 0,
            },
        );
        assert_eq!(spans, names.map(which).to_vec());
    }
}

/// Two needs of one name: its deciding question reads `a`, which lies in its knot, and `b`, which
/// reaches the knot only through the name. All three are rigid from every start.
#[test]
fn a_need_reaching_a_knot_only_through_its_name_joins_it() {
    for probes_first in [false, true] {
        let Looped {
            mut case,
            body,
            x,
            call,
        } = looped();
        let decided = case.text.answered_name();
        let a = case.text.answered_name();
        let b = case.text.answered_name();
        case.text.answers_through(call, a, decided);
        case.text.answers_through(call, b, decided);
        let entry = Site::Entry(body);
        let mut probes = Vec::new();
        if probes_first {
            probes = [decided, a, b]
                .map(|name| probe(&mut case, entry, name, Probe::Which))
                .to_vec();
        }
        let decide = case.ask(entry, &[a, b], &[(a, Probe::Which)], 2);
        case.text.answers(decide, 0, decided, atom(0));
        case.text.answers(decide, 1, decided, atom(1));
        if !probes_first {
            probes = [decided, a, b]
                .map(|name| probe(&mut case, entry, name, Probe::Which))
                .to_vec();
        }
        let (graph, mut answers) = case.lay();
        let spans = from_each_start(
            &graph,
            &mut answers,
            body,
            x,
            &probes,
            Stacked {
                demands: 4,
                knots_of_one_type: 0,
                other_knots: 1,
            },
        );
        assert_eq!(spans, [decided, a, b].map(which).to_vec());
    }
}

/// The knot `n`, `q`, `v`, `w`: `n`'s deciding question `q` reads `v`, `v` takes `w` and `w`
/// takes `n`. `n` takes no arm and is its own rigid. `u2` takes `n` once it is final, so it is
/// `n`'s rigid and off the knot, and `t` is typed by `qt`'s arm on `u2`. All of it holds from a
/// start at `n` and from a start at `q`.
#[test]
fn a_name_off_a_knot_reads_its_final_rigid() {
    for start_at_n in [true, false] {
        let Looped {
            mut case,
            body,
            x,
            call,
        } = looped();
        let entry = Site::Entry(body);
        let [n, v, w, t, u2] = [(); 5].map(|_| case.text.answered_name());
        let first = start_at_n.then(|| probe(&mut case, entry, n, Probe::Which));
        let q = case.ask(entry, &[v], &[(v, Probe::Which)], 2);
        case.text.answers(q, 0, n, atom(0));
        case.text.answers(q, 1, n, atom(1));
        case.text.answers_through(call, v, w);
        case.text.answers_through(call, w, n);
        case.text.answers_through(call, u2, n);
        let qt = case.ask(entry, &[u2], &[(u2, Probe::Which)], 3);
        for arm in 0..3 {
            case.text.answers(qt, arm, t, atom(10 + arm as u8));
        }
        let probe_n = first.unwrap_or_else(|| probe(&mut case, entry, n, Probe::Which));
        let mut probes = vec![probe_n];
        for (name, how) in [(v, Probe::Which), (w, Probe::Which), (u2, Probe::Which)] {
            probes.push(probe(&mut case, entry, name, how));
        }
        probes.push(probe(&mut case, entry, t, Probe::Leaf));
        let (graph, mut answers) = case.lay();
        let spans = from_each_start(
            &graph,
            &mut answers,
            body,
            x,
            &probes,
            Stacked {
                demands: 4,
                knots_of_one_type: 0,
                other_knots: 1,
            },
        );
        let arm = n.index() as u32 % 3;
        let expected = vec![
            which(n),
            which(v),
            which(w),
            which(n),
            Span::exact(10 + arm),
        ];
        assert_eq!(spans, expected);
    }
}

/// Names `a`, `b`, `c` have deciding questions reading `{b', c'}`, `{a'}` and `{b'}`, and each
/// primed name takes its unprimed one through the self-call. From a start at `a`, `c`'s question
/// reaches `b'` done but its knot unclosed. All six names are rigid, from every start.
#[test]
fn a_cross_edge_into_an_unclosed_knot_joins_it() {
    for first in 0..3 {
        let Looped {
            mut case,
            body,
            x,
            call,
        } = looped();
        let entry = Site::Entry(body);
        let [a, b, c, a1, b1, c1] = [(); 6].map(|_| case.text.answered_name());
        for (primed, plain) in [(a1, a), (b1, b), (c1, c)] {
            case.text.answers_through(call, primed, plain);
        }
        let deciding = [(a, vec![b1, c1]), (b, vec![a1]), (c, vec![b1])];
        for offset in 0..3 {
            let (decided, reads) = &deciding[(first + offset) % 3];
            let question = case.ask(entry, reads, &[(reads[0], Probe::Which)], 2);
            case.text.answers(question, 0, *decided, atom(0));
            case.text.answers(question, 1, *decided, atom(1));
        }
        let names = [a, b, c, a1, b1, c1];
        let probes = names.map(|name| probe(&mut case, entry, name, Probe::Which));
        let (graph, mut answers) = case.lay();
        let spans = from_each_start(
            &graph,
            &mut answers,
            body,
            x,
            &probes,
            Stacked {
                demands: 9,
                knots_of_one_type: 0,
                other_knots: 1,
            },
        );
        assert_eq!(spans, names.map(which).to_vec());
    }
}

/// `a` takes `n` through the self-call, and `n` takes `p` through a call passing `a`, so `a` and
/// `n` knot. `n` takes nothing: the callee under `x := a` is entered by the follow once `a` is
/// final, and its `p`, taking `a` from the load's view, is `a`'s rigid. It joins no knot.
#[test]
fn a_name_whose_call_passes_a_knotted_name_takes_nothing() {
    let Looped {
        mut case,
        body,
        x,
        call,
    } = looped();
    let entry = Site::Entry(body);
    let [a, n, p] = [(); 3].map(|_| case.text.answered_name());
    case.text.answers_through(call, a, n);
    let passing = case.text.statement(body);
    let passing = case.call(passing, body, vec![(x, name(a))]);
    case.text.answers_through(passing, n, p);
    let always = case.ask(entry, &[x], &[], 1);
    let fixed = case.text.passes(always, 0, body, vec![(x, atom(NUMBER))]);
    case.text.answers_through(fixed, p, a);
    let probes = [a, n, p].map(|name| probe(&mut case, entry, name, Probe::Which));
    let (graph, mut answers) = case.lay();
    assert_eq!(
        from_each_start(
            &graph,
            &mut answers,
            body,
            x,
            &probes,
            Stacked {
                demands: 6,
                knots_of_one_type: 0,
                other_knots: 2
            }
        ),
        vec![which(a), which(n), which(a)]
    );
    let bound_bump = Bump::new();
    let mut bound = Kept::new(&graph, &bound_bump);
    bound.instance(graph.root(), &Binding::new(), &mut answers);
    let passed = bind(&[(x, name(a))]);
    assert_eq!(
        answered(&mut bound, probes[2], &passed, &mut answers),
        which(a)
    );
}

/// Two knots of takes, one after the other: a name off both reads final rigids from each.
#[test]
fn a_name_after_two_knots_reads_both_rigids() {
    let Looped {
        mut case,
        body,
        x,
        call,
    } = looped();
    let entry = Site::Entry(body);
    let [a, a1, b, b1, m] = [(); 5].map(|_| case.text.answered_name());
    for (taker, taken) in [(a, a1), (a1, a), (b, b1), (b1, b)] {
        case.text.answers_through(call, taker, taken);
    }
    let decide = case.ask(entry, &[a, b], &[(a, Probe::Which), (b, Probe::Which)], 2);
    case.text.answers(decide, 0, m, atom(20));
    case.text.answers(decide, 1, m, atom(21));
    let probes = [
        probe(&mut case, entry, a, Probe::Which),
        probe(&mut case, entry, b1, Probe::Which),
        probe(&mut case, entry, m, Probe::Leaf),
    ];
    let (graph, mut answers) = case.lay();
    let spans = from_each_start(
        &graph,
        &mut answers,
        body,
        x,
        &probes,
        Stacked {
            demands: 4,
            knots_of_one_type: 2,
            other_knots: 0,
        },
    );
    let arm = (a.index() + b.index()) as u32 % 2;
    assert_eq!(spans, vec![which(a), which(b1), Span::exact(20 + arm)]);
}

/// `SPIN`: `L`'s `result`, a `MATCH`, is typed on a site as the union of `BOX x`'s module and
/// `rest`, which takes `result` through the tail call under the same view. The two knot, and
/// the knot carries one type: `BOX x`'s module under each view of `x`. The root reads the
/// `Number` `BOX`'s `sample`, and the knot's take is walked once.
#[test]
fn spin_types_its_tail_loop_by_the_arm_that_ends_it() {
    let Looped {
        mut case,
        body,
        x,
        call,
    } = looped();
    let boxed = boxed(&mut case, body, x);
    let result = case.text.answered_name();
    let rest = case.text.answered_name();
    case.text.answers_through(call, rest, result);
    let matched = case.text.statement(body);
    case.text
        .answers_at(matched, result, union(&[name(boxed), name(rest)]));
    let shown = [result, rest].map(|typed| probe(&mut case, Site::Entry(body), typed, Probe::Leaf));
    // The root's `(SPIN 5 3).sample`.
    let root = case.text.root();
    let top = case.text.statement(root);
    let spun_call = case.call(top, body, vec![(x, atom(NUMBER))]);
    let spun = case.text.answered_name();
    case.text.answers_through(spun_call, spun, result);
    let sample = probe(&mut case, top, spun, Probe::Leaf);
    let (graph, mut answers) = case.lay();
    let entry = graph.entry(body);
    assert_eq!(graph.answered_at(result), Some(entry));
    assert!(graph.name_component(result).is_some());
    assert_eq!(graph.name_component(result), graph.name_component(rest));
    assert_eq!(graph.name_component(boxed), None);

    let stacked = Stacked {
        demands: 2,
        knots_of_one_type: 1,
        other_knots: 0,
    };
    let spans = from_each_start(&graph, &mut answers, body, x, &shown, stacked);
    assert_eq!(spans, vec![Span::exact(u32::from(NUMBER)); 2]);

    let root_bump = Bump::new();
    let mut from_root = Kept::new(&graph, &root_bump);
    from_root.instance(graph.root(), &Binding::new(), &mut answers);
    let read = answered(&mut from_root, sample, &Binding::new(), &mut answers);
    assert_eq!(read, Span::exact(u32::from(NUMBER)));
    // `spun`'s take, `rest`'s take, the root's two calls and `L`'s tail call followed.
    assert_eq!(from_root.bound.walked(), 5);

    let body_bump = Bump::new();
    let mut from_body = Kept::new(&graph, &body_bump);
    from_body.instance(entry, &bind(&[(x, atom(NUMBER))]), &mut answers);
    // `rest`'s take through the tail call, once, and the tail call followed.
    assert_eq!(from_body.bound.walked(), 2);

    let str_bump = Bump::new();
    let mut under_str = Kept::new(&graph, &str_bump);
    let view = bind(&[(x, atom(STR))]);
    under_str.instance(entry, &view, &mut answers);
    let read = answered(&mut under_str, shown[0], &view, &mut answers);
    assert_eq!(read, Span::exact(u32::from(STR)));
}

/// `L` whose `MATCH` has a fixed `Str` arm beside `BOX x` and the tail call: the knot carries
/// the union of both arms that end it, so each name reads both leaves.
#[test]
fn a_tail_loop_unions_every_arm_that_ends_it() {
    let Looped {
        mut case,
        body,
        x,
        call,
    } = looped();
    let boxed = boxed(&mut case, body, x);
    let result = case.text.answered_name();
    let rest = case.text.answered_name();
    case.text.answers_through(call, rest, result);
    let matched = case.text.statement(body);
    let arms = union(&[name(boxed), atom(STR), name(rest)]);
    case.text.answers_at(matched, result, arms);
    let shown = [result, rest].map(|typed| probe(&mut case, Site::Entry(body), typed, Probe::Leaf));
    let (graph, mut answers) = case.lay();
    let stacked = Stacked {
        demands: 2,
        knots_of_one_type: 1,
        other_knots: 0,
    };
    let spans = from_each_start(&graph, &mut answers, body, x, &shown, stacked);
    let both = Span {
        lo: u32::from(NUMBER),
        hi: u32::from(STR),
    };
    assert_eq!(spans, vec![both; 2]);
}

/// `L` whose `MATCH` has two arms, each a tail call: the knot carries one type but reaches no
/// arm from outside, so each name is its own rigid.
#[test]
fn a_tail_loop_with_no_way_out_keeps_its_rigids() {
    let Looped {
        mut case,
        body,
        x,
        call,
    } = looped();
    let [result, left, right] = [(); 3].map(|_| case.text.answered_name());
    let second = case.text.statement(body);
    let again = case.call(second, body, vec![(x, name(x))]);
    case.text.answers_through(call, left, result);
    case.text.answers_through(again, right, result);
    let matched = case.text.statement(body);
    case.text
        .answers_at(matched, result, union(&[name(left), name(right)]));
    let names = [result, left, right];
    let probes = names.map(|typed| probe(&mut case, Site::Entry(body), typed, Probe::Which));
    let (graph, mut answers) = case.lay();
    let stacked = Stacked {
        demands: 3,
        knots_of_one_type: 1,
        other_knots: 0,
    };
    let spans = from_each_start(&graph, &mut answers, body, x, &probes, stacked);
    assert_eq!(spans, names.map(which).to_vec());
}

/// `L` whose `n` takes `m` through the self-call and `m` takes `d`, which a question reading `n`
/// decides, taking no arm while `n` is rigid. Once `L 0` rests, a call from `L 2`'s entry into
/// `L 0`, passing a name typed `0` through `x`, takes `m`: the filed evaluation's question is
/// final at once, yet `d` still reads `n`, so `m` lies in the knot and is its own rigid, as from a
/// fresh start.
#[test]
fn a_filed_question_keeps_its_knot() {
    let Looped {
        mut case,
        body,
        x,
        call,
    } = looped();
    let entry = Site::Entry(body);
    let [n, m, d, zero, taken] = [(); 5].map(|_| case.text.answered_name());
    case.text.answers_through(call, n, m);
    case.text.answers_through(call, m, d);
    let decide = case.ask(Site::Statement(body, 0), &[n], &[(n, Probe::Leaf)], 1);
    case.text.answers(decide, 0, d, atom(0));
    // `0` read through `x`, so the call is laid in `L`.
    let always = case.ask(entry, &[x], &[], 1);
    case.text.answers(always, 0, zero, atom(0));
    let into = case.call(entry, body, vec![(x, name(zero))]);
    case.text.answers_through(into, taken, m);
    let shown = probe(&mut case, entry, taken, Probe::Which);
    let (graph, mut answers) = case.lay();
    let owner = graph.question(shown).owner();
    let view = bind(&[(x, atom(2))]);
    for rested in [false, true] {
        let bump = Bump::new();
        let mut kept = Kept::new(&graph, &bump);
        if rested {
            kept.instance(owner, &bind(&[(x, atom(0))]), &mut answers);
        }
        kept.instance(owner, &view, &mut answers);
        assert_eq!(answered(&mut kept, shown, &view, &mut answers), which(m));
    }
}

/// `L` whose `MATCH`'s tail arm wraps the tail call's result in a list: a built type on the
/// knot, so it carries no one type and each name is its own rigid.
#[test]
fn a_union_building_on_its_knot_keeps_its_rigids() {
    let Looped {
        mut case,
        body,
        x,
        call,
    } = looped();
    let boxed = boxed(&mut case, body, x);
    let result = case.text.answered_name();
    let rest = case.text.answered_name();
    case.text.answers_through(call, rest, result);
    let matched = case.text.statement(body);
    let arms = union(&[name(boxed), list(name(rest))]);
    case.text.answers_at(matched, result, arms);
    let names = [result, rest];
    let probes = names.map(|typed| probe(&mut case, Site::Entry(body), typed, Probe::Which));
    let (graph, mut answers) = case.lay();
    let stacked = Stacked {
        demands: 2,
        knots_of_one_type: 0,
        other_knots: 1,
    };
    let spans = from_each_start(&graph, &mut answers, body, x, &probes, stacked);
    assert_eq!(spans, names.map(which).to_vec());
}

/// `Q`/`P`: `P`'s `S` takes `Q`'s result through a call on `P`'s entry, and is typed alike from
/// the root and from `P`'s entry.
#[test]
fn a_result_is_taken_alike_from_either_start() {
    let mut case = Case::new();
    let root = case.text.root();
    let q = case.text.body(root);
    let q_in = case.text.name(Site::Entry(q));
    let q_result = case.text.answered_name();
    let decide = case.ask(Site::Entry(q), &[q_in], &[(q_in, Probe::Leaf)], 2);
    case.text.answers(decide, 0, q_result, atom(7));
    case.text.answers(decide, 1, q_result, atom(8));
    let p = case.text.body(root);
    let p_in = case.text.name(Site::Entry(p));
    let call = case.call(Site::Entry(p), q, vec![(q_in, name(p_in))]);
    let s = case.text.answered_name();
    case.text.answers_through(call, s, q_result);
    let shown = probe(&mut case, Site::Entry(p), s, Probe::Leaf);
    let top = case.text.statement(root);
    case.call(top, p, vec![(p_in, atom(1))]);
    let (graph, mut answers) = case.lay();
    let view = bind(&[(p_in, atom(1))]);
    let from_root_bump = Bump::new();
    let mut from_root = Kept::new(&graph, &from_root_bump);
    from_root.instance(graph.root(), &Binding::new(), &mut answers);
    let from_p_bump = Bump::new();
    let mut from_p = Kept::new(&graph, &from_p_bump);
    from_p.instance(graph.entry(p), &view, &mut answers);
    assert_eq!(
        answered(&mut from_root, shown, &view, &mut answers),
        Span::exact(8)
    );
    assert_eq!(
        answered(&mut from_p, shown, &view, &mut answers),
        Span::exact(8)
    );
}

/// A content that cannot be computed over open content leaves the answered name its rigid.
#[test]
fn an_arm_content_that_cannot_apply_is_rigid() {
    let mut case = Case::new();
    let body = case.text.body(case.text.root());
    let x = case.text.name(Site::Entry(body));
    let peeled = case.text.answered_name();
    // An empty plan totals zero, so the question takes its one arm whatever `x` is.
    let decide = case.ask(Site::Entry(body), &[x], &[], 1);
    case.text.answers(decide, 0, peeled, peel(name(x)));
    let shown = probe(&mut case, Site::Entry(body), peeled, Probe::Which);
    let (graph, mut answers) = case.lay();
    let bound_bump = Bump::new();
    let mut bound = Kept::new(&graph, &bound_bump);
    bound.instance(graph.entry(body), &Binding::new(), &mut answers);
    assert_eq!(
        answered(&mut bound, shown, &Binding::new(), &mut answers),
        which(peeled)
    );
}
