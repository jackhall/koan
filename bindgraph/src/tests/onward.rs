//! Onward calls: a holder instanced from the evaluation that follows its call where its names
//! arrive, bound by the run otherwise, and a name taken from a callee's holder through the chain
//! of onward calls that reaches it.

use bumpstorage::Bump;

use super::cases::{Case, bind};
use super::harness::{Counting, Kept};
use super::toy::{Probe, atom, list, name};
use crate::{Binding, Site};

/// `PICK`: its result is decided by its `x`'s depth.
fn pick(case: &mut Case) -> (crate::BodyId, crate::NameId, crate::NameId) {
    let root = case.text.root();
    let pick = case.text.body(root);
    let x = case.text.name(Site::Entry(pick));
    let result = case.text.answered_name();
    let boxed = case.ask(Site::Entry(pick), &[x], &[(x, Probe::Depth)], 2);
    case.text.answers(boxed, 0, result, atom(1));
    case.text.answers(boxed, 1, result, list(atom(1)));
    (pick, x, result)
}

/// `FIXED`: `LET held = (PICK 5)`, then `SHOW held`. The root owns the onward call, and making
/// the root's instance makes `LET held`'s under the answer; a run binding the same content finds
/// it and runs nothing.
#[test]
fn a_text_fixed_holder_is_instanced_from_the_root() {
    let mut case = Case::new();
    let root = case.text.root();
    let (pick, x, result) = pick(&mut case);
    let fixed = case.text.body(root);
    let first = case.text.statement(fixed);
    let call = case.call(first, pick, vec![(x, atom(5))]);
    let twin = case.text.answered_name();
    case.text.answers_through(call, twin, result);
    let held = case.text.name(first);
    let onward = case.text.arrives(first, vec![(held, name(twin))]);
    let second = case.text.statement(fixed);
    let show = case.ask(second, &[held], &[(held, Probe::Depth)], 0);
    let (graph, mut answers) = case.lay();
    let node = graph.node_at(first).expect("the holder is a bind site");
    assert_eq!(graph.question(show).owner(), node);
    assert_eq!(graph.edge(onward).owner(), graph.root());
    assert_eq!(graph.node(graph.root()).answered(), &[twin]);

    let bump = Bump::new();
    let mut kept = Kept::new(&graph, &bump);
    kept.instance(graph.root(), &Binding::new(), &mut answers);
    let run = bind(&[(held, atom(1))]);
    let found = kept
        .found(node, &run, &mut answers)
        .expect("the load made it");
    assert_eq!(
        found,
        graph.afresh(&super::toy::Toy, node, &run, &mut answers)
    );
    let walked = kept.bound.walked();
    let mut counting = Counting::new(&mut answers);
    kept.instance(node, &run, &mut counting);
    assert!(counting.runs.is_empty());
    assert_eq!(kept.bound.walked(), walked);
    // Another content is the run's to make.
    assert!(
        kept.found(node, &bind(&[(held, list(atom(1)))]), &mut answers)
            .is_none()
    );
}

/// `USE`: `LET held = (PICK <open>)`. No answer types it, so the load makes no instance at the
/// holder, and two runs binding one content share a handle there.
#[test]
fn a_holder_that_waits_on_the_run_is_bound_and_shared_there() {
    let mut case = Case::new();
    let root = case.text.root();
    let (pick, _, result) = pick(&mut case);
    let user = case.text.body(root);
    let first = case.text.statement(user);
    let call = case.call(first, pick, vec![]);
    let twin = case.text.answered_name();
    case.text.answers_through(call, twin, result);
    let held = case.text.name(first);
    case.text.arrives(first, vec![(held, name(twin))]);
    let second = case.text.statement(user);
    case.ask(second, &[held], &[(held, Probe::Depth)], 0);
    let (graph, mut answers) = case.lay();
    let node = graph.node_at(first).expect("the holder is a bind site");

    let bump = Bump::new();
    let mut kept = Kept::new(&graph, &bump);
    kept.instance(graph.root(), &Binding::new(), &mut answers);
    assert_eq!(kept.resting_at(node), 0);
    let run = bind(&[(held, atom(1))]);
    let one = kept.instance(node, &run, &mut answers);
    let mut counting = Counting::new(&mut answers);
    let two = kept.instance(node, &run, &mut counting);
    assert_eq!(one, two);
    assert!(counting.runs.is_empty());
    assert_eq!(kept.resting_at(node), 1);
}

/// `USE2 y :Item`: `LET held = (PICK y)`, then a question over `held` and `Item`. The entry owns
/// the onward call, and its instance under `Item := 5` makes the holder's under both.
#[test]
fn a_holder_under_the_callers_name_is_instanced_from_its_entry() {
    let mut case = Case::new();
    let root = case.text.root();
    let (pick, x, result) = pick(&mut case);
    let user = case.text.body(root);
    let item = case.text.name(Site::Entry(user));
    let first = case.text.statement(user);
    let call = case.call(first, pick, vec![(x, name(item))]);
    let twin = case.text.answered_name();
    case.text.answers_through(call, twin, result);
    let held = case.text.name(first);
    let onward = case.text.arrives(first, vec![(held, name(twin))]);
    let second = case.text.statement(user);
    let plan = [(held, Probe::Depth), (item, Probe::Depth)];
    case.ask(second, &[held, item], &plan, 0);
    let top = case.text.statement(root);
    case.call(top, user, vec![(item, list(atom(5)))]);
    let (graph, mut answers) = case.lay();
    let node = graph.node_at(first).expect("the holder is a bind site");
    assert_eq!(graph.edge(onward).owner(), graph.entry(user));

    let bump = Bump::new();
    let mut kept = Kept::new(&graph, &bump);
    kept.instance(graph.root(), &Binding::new(), &mut answers);
    let run = bind(&[(item, list(atom(5))), (held, list(atom(1)))]);
    let found = kept
        .found(node, &run, &mut answers)
        .expect("the load made it");
    assert_eq!(
        found,
        graph.afresh(&super::toy::Toy, node, &run, &mut answers)
    );
    // A run binding `Item` anew at the entry makes the holder's instance with it.
    let other = bind(&[(item, atom(7))]);
    kept.instance(graph.entry(user), &other, &mut answers);
    let run = bind(&[(item, atom(7)), (held, atom(1))]);
    assert!(kept.found(node, &run, &mut answers).is_some());
}

/// `LET a = (PICK 5)`, `LET b = (PICK a)`, then a question over `b`: the second onward call is
/// owned at the first holder, and the root's instance reaches both.
#[test]
fn holders_chain_through_their_bind_sites() {
    let mut case = Case::new();
    let root = case.text.root();
    let (pick, x, result) = pick(&mut case);
    let fixed = case.text.body(root);
    let first = case.text.statement(fixed);
    let call = case.call(first, pick, vec![(x, list(atom(5)))]);
    let twin_a = case.text.answered_name();
    case.text.answers_through(call, twin_a, result);
    let a = case.text.name(first);
    case.text.arrives(first, vec![(a, name(twin_a))]);
    let second = case.text.statement(fixed);
    let call = case.call(second, pick, vec![(x, name(a))]);
    let twin_b = case.text.answered_name();
    case.text.answers_through(call, twin_b, result);
    let b = case.text.name(second);
    let onward = case.text.arrives(second, vec![(b, name(twin_b))]);
    let third = case.text.statement(fixed);
    case.ask(third, &[b], &[(b, Probe::Depth)], 0);
    let (graph, mut answers) = case.lay();
    let (at_a, at_b) = (
        graph.node_at(first).unwrap(),
        graph.node_at(second).unwrap(),
    );
    assert_eq!(graph.edge(onward).owner(), at_a);
    assert_eq!(graph.node(at_a).answered(), &[twin_b]);

    let bump = Bump::new();
    let mut kept = Kept::new(&graph, &bump);
    kept.instance(graph.root(), &Binding::new(), &mut answers);
    // `PICK [5]` is `[1]`, and `PICK [1]` is `[1]`.
    let run = bind(&[(a, list(atom(1))), (b, list(atom(1)))]);
    assert!(kept.found(at_b, &run, &mut answers).is_some());
}

/// `SPIN x n = MATCH n WITH {0: (PICK x), Number: (SPIN x (n - 1))}`: its result is the union of
/// what `PICK x` gives and its own through the recursive call, a knot that carries one type. A
/// holder of `SPIN 5 3` is instanced from the root under that type.
#[test]
fn a_holder_of_a_tail_loop_is_instanced_under_the_arm_that_ends_it() {
    use super::toy::union;
    let mut case = Case::new();
    let root = case.text.root();
    let (pick, x, result) = pick(&mut case);
    let spin = case.text.body(root);
    let elem = case.text.name(Site::Entry(spin));
    let only = case.text.statement(spin);
    let ends = case.call(only, pick, vec![(x, name(elem))]);
    let ended = case.text.answered_name();
    case.text.answers_through(ends, ended, result);
    let again = case.call(only, spin, vec![(elem, name(elem))]);
    let looped = case.text.answered_name();
    let spun = case.text.answered_name();
    case.text.answers_through(again, looped, spun);
    case.text
        .answers_at(only, spun, union(&[name(ended), name(looped)]));

    let fixed = case.text.body(root);
    let first = case.text.statement(fixed);
    let call = case.call(first, spin, vec![(elem, list(atom(5)))]);
    let twin = case.text.answered_name();
    case.text.answers_through(call, twin, spun);
    let held = case.text.name(first);
    case.text.arrives(first, vec![(held, name(twin))]);
    let second = case.text.statement(fixed);
    case.ask(second, &[held], &[(held, Probe::Depth)], 0);
    let (graph, mut answers) = case.lay();
    let node = graph.node_at(first).expect("the holder is a bind site");
    assert!(graph.name_component(spun).is_some());

    let bump = Bump::new();
    let mut kept = Kept::new(&graph, &bump);
    kept.instance(graph.root(), &Binding::new(), &mut answers);
    let run = bind(&[(held, list(atom(1)))]);
    assert!(kept.found(node, &run, &mut answers).is_some());
}

/// `MID z = (LET first = (PICK z); BOX first)`, `FIXED = (LET held = (MID [5]); SHOW held)`:
/// `MID`'s result is decided at `LET first`'s node. `open` leaves `PICK`'s `x` unmapped, so
/// `first` waits on the run.
fn mid(
    open: bool,
) -> (
    crate::Graph<super::toy::Toy>,
    super::toy::ToyAnswers,
    [crate::NodeId; 2],
    crate::NameId,
) {
    let mut case = Case::new();
    let root = case.text.root();
    let (pick, x, result) = pick(&mut case);
    let mid = case.text.body(root);
    let z = case.text.name(Site::Entry(mid));
    let first_at = case.text.statement(mid);
    let mapping = if open { vec![] } else { vec![(x, name(z))] };
    let call = case.call(first_at, pick, mapping);
    let first_twin = case.text.answered_name();
    case.text.answers_through(call, first_twin, result);
    let first = case.text.name(first_at);
    case.text.arrives(first_at, vec![(first, name(first_twin))]);
    let last = case.text.statement(mid);
    let boxed = case.ask(last, &[first], &[(first, Probe::Depth)], 2);
    let mid_result = case.text.answered_name();
    case.text.answers(boxed, 0, mid_result, atom(2));
    case.text.answers(boxed, 1, mid_result, list(atom(2)));

    let fixed = case.text.body(root);
    let held_at = case.text.statement(fixed);
    let call = case.call(held_at, mid, vec![(z, list(atom(5)))]);
    let twin = case.text.answered_name();
    case.text.answers_through(call, twin, mid_result);
    let held = case.text.name(held_at);
    case.text.arrives(held_at, vec![(held, name(twin))]);
    let second = case.text.statement(fixed);
    case.ask(second, &[held], &[(held, Probe::Depth)], 0);
    let (graph, answers) = case.lay();
    let nodes = [
        graph.node_at(first_at).unwrap(),
        graph.node_at(held_at).unwrap(),
    ];
    assert_eq!(graph.answered_at(mid_result), Some(nodes[0]));
    (graph, answers, nodes, held)
}

#[test]
fn a_result_bound_at_a_callees_holder_is_taken_where_onward_calls_reach_it() {
    let (graph, mut answers, [_, at_held], held) = mid(false);
    let bump = Bump::new();
    let mut kept = Kept::new(&graph, &bump);
    kept.instance(graph.root(), &Binding::new(), &mut answers);
    let run = bind(&[(held, list(atom(2)))]);
    let found = kept
        .found(at_held, &run, &mut answers)
        .expect("the load made it");
    assert_eq!(
        found,
        graph.afresh(&super::toy::Toy, at_held, &run, &mut answers)
    );
    let reached = super::reference::reference(&graph, &mut answers, graph.root(), &Binding::new());
    assert!(reached.reached.iter().any(|each| each.node == at_held));
}

#[test]
fn a_result_bound_at_a_callees_holder_that_waits_on_the_run_is_a_rigid() {
    let (graph, mut answers, [at_first, at_held], _) = mid(true);
    let bump = Bump::new();
    let mut kept = Kept::new(&graph, &bump);
    kept.instance(graph.root(), &Binding::new(), &mut answers);
    assert_eq!(kept.resting_at(at_first), 0);
    assert_eq!(kept.resting_at(at_held), 0);
}

/// `LET held = (PICK 5)`, given a fresh body and the call's result as its arrival, and a reader.
fn held(case: &mut Case) -> (Site, crate::NameId, crate::NameId) {
    let root = case.text.root();
    let (pick, x, result) = pick(case);
    let user = case.text.body(root);
    let first = case.text.statement(user);
    let call = case.call(first, pick, vec![(x, atom(5))]);
    let twin = case.text.answered_name();
    case.text.answers_through(call, twin, result);
    let held = case.text.name(first);
    let second = case.text.statement(user);
    case.ask(second, &[held], &[(held, Probe::Depth)], 0);
    (first, held, twin)
}

#[test]
fn an_onward_call_mapping_a_name_its_statement_does_not_bind_is_malformed() {
    let mut case = Case::new();
    let (first, _, twin) = held(&mut case);
    let elsewhere = case.text.name(Site::Entry(first.body()));
    let edge = case.text.arrives(first, vec![(elsewhere, name(twin))]);
    assert_eq!(
        case.refusal(),
        crate::LayError::Malformed(crate::Malformed::MapsUnboundName {
            edge,
            name: elsewhere
        })
    );
}

#[test]
fn an_onward_call_reading_the_name_it_arrives_at_is_malformed() {
    let mut case = Case::new();
    let (first, held, _) = held(&mut case);
    let edge = case.text.arrives(first, vec![(held, list(name(held)))]);
    assert_eq!(
        case.refusal(),
        crate::LayError::Malformed(crate::Malformed::ArrivesThroughItself { edge })
    );
}

#[test]
fn a_second_onward_call_into_one_statement_is_malformed() {
    let mut case = Case::new();
    let (first, held, twin) = held(&mut case);
    case.text.arrives(first, vec![(held, name(twin))]);
    let edge = case.text.arrives(first, vec![(held, list(name(twin)))]);
    assert_eq!(
        case.refusal(),
        crate::LayError::Malformed(crate::Malformed::ArrivesTwice { edge })
    );
}
