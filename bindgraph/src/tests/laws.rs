//! The crate's laws, each a property over generated texts and bindings.
//!
//! The oracle for an instance is the same graph answered from scratch ([`Graph::afresh`]) under
//! the full binding the [reference](super::reference) evaluation reaches: the view with every
//! answered name typed, and for a handle the reference's [`admission`]. Law 1 also checks
//! ownership and placement against bases and a linear order of sites the toy builds itself, and
//! the growth law checks the refusal against [`grows_by_terms`], which composes terms by brute
//! force. The knot law reads each knot the reference resolves against the text components as
//! laid. A text the growth check refuses is vacuous for the others; [`generators_bite`] pins that
//! the generators lay and refuse texts, and exercise every kind of edge, knot, ask and borrowed
//! question often enough to matter.

use std::collections::BTreeSet;

use bumpstorage::Bump;
use proptest::prelude::*;
use proptest::strategy::ValueTree;
use proptest::test_runner::TestRunner;

use super::case_share;
use super::harness::{Counting, Kept, handle, read};
use super::reference::{Reference, Unknown, admission, reference};
use super::toy::{
    Draw, Term, Toy, ToyAnswers, ToyText, Verdict, atom, bases, grows_by_terms, linear_order, list,
    node, placed, raw, site, toy_binding, toy_binding_with_answered, toy_text,
};
use crate::{
    Binding, Bound, ComponentId, EdgeId, Graph, LayError, NameId, NodeId, QuestionId, Resting,
    Source,
};

fn config() -> ProptestConfig {
    ProptestConfig {
        cases: case_share(1, 1),
        ..ProptestConfig::default()
    }
}

/// The graph laid from `text` with its answer function, or `None` where the growth check refuses
/// it. Generated texts are never malformed.
pub(super) fn laid(text: &ToyText) -> Option<(Graph<Toy>, ToyAnswers)> {
    let (built, answers) = text.build();
    match built.lay(&Toy) {
        Ok(graph) => Some((graph, answers)),
        Err(LayError::Grows { .. }) => None,
        Err(malformed) => panic!("a generated text is malformed: {malformed}"),
    }
}

/// Every node, in `NodeId` order.
fn nodes(graph: &Graph<Toy>) -> impl Iterator<Item = NodeId> + use<> {
    (0..graph.nodes().len()).map(NodeId::new)
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

proptest! {
    #![proptest_config(config())]

    /// One owner: each question and call is owned by exactly one node, and each answered name
    /// bound at one, the latest site of its base in an order built here. The owner's instance
    /// under only what is bound up to the owner answers as the oracle.
    #[test]
    fn one_owner(raw in raw()) {
        let mut draw = Draw::new(raw);
        let text = toy_text(&mut draw, false);
        let Some((graph, mut answers)) = laid(&text) else { return Ok(()) };
        let bases = bases(&text);
        let position = linear_order(&text, &bases);
        let latest = |base: &BTreeSet<usize>| {
            let at = base.iter().map(|&name| text.names[name]).max_by_key(|at| position[at]);
            site(at.unwrap_or((0, None)))
        };
        for (index, question) in text.questions.iter().enumerate() {
            let id = QuestionId::new(index);
            let owners: Vec<NodeId> = nodes(&graph).filter(|&node| graph.node(node).owns().contains(&id)).collect();
            prop_assert_eq!(owners.len(), 1);
            let base: BTreeSet<usize> = question.reads.iter().flat_map(|&read| bases.names[read].iter().copied()).collect();
            prop_assert_eq!(graph.node(owners[0]).site(), latest(&base));
            prop_assert_eq!(graph.question(id).owner(), owners[0]);
        }
        for index in 0..text.edges.len() {
            let id = EdgeId::new(index);
            let owners: Vec<NodeId> = nodes(&graph).filter(|&node| graph.node(node).follows().contains(&id)).collect();
            prop_assert_eq!(owners.len(), 1);
            prop_assert_eq!(graph.node(owners[0]).site(), latest(&bases.inputs[index]));
            prop_assert_eq!(graph.edge(id).owner(), owners[0]);
        }
        let placed = placed(&text, &bases, &position);
        for (name, &bound_at) in placed.iter().enumerate().skip(text.names.len()) {
            let id = NameId::new(name);
            let at: Vec<NodeId> = nodes(&graph).filter(|&node| graph.node(node).answered().contains(&id)).collect();
            prop_assert_eq!(at.len(), 1);
            prop_assert_eq!(graph.node(at[0]).site(), site(bound_at));
        }
        let binding = toy_binding(&mut draw, text.names.len());
        let bump = Bump::new();
        let mut kept = Kept::new(&graph, &bump);
        for node in nodes(&graph) {
            let up_to = graph.bound_at(node, &binding);
            let read = kept.answers(node, &up_to, &mut answers);
            let expected = expected(&graph, &mut answers, node, &up_to);
            prop_assert_eq!(read, expected);
        }
    }

    /// Bound equals declared: every instance a sequence of calls returns answers as the oracle,
    /// under bindings that may type answered names too, which both take as given.
    #[test]
    fn bound_equals_declared(raw in raw()) {
        let mut draw = Draw::new(raw);
        let text = toy_text(&mut draw, false);
        let Some((graph, mut answers)) = laid(&text) else { return Ok(()) };
        let bump = Bump::new();
        let mut kept = Kept::new(&graph, &bump);
        for _ in 0..1 + draw.pick(6) {
            let node = NodeId::new(draw.pick(graph.nodes().len()));
            let binding = toy_binding_with_answered(&mut draw, &text);
            let read = kept.answers(node, &binding, &mut answers);
            let expected = expected(&graph, &mut answers, node, &binding);
            prop_assert_eq!(read, expected);
        }
    }

    /// Order does not matter: binding a body's nodes under a widening chain of bindings, then
    /// under the whole, reaches the answers a fresh graph reaches under the whole at once, in
    /// reverse node order.
    #[test]
    fn order_does_not_matter(raw in raw()) {
        let mut draw = Draw::new(raw);
        let text = toy_text(&mut draw, false);
        let Some((graph, mut answers)) = laid(&text) else { return Ok(()) };
        let body = draw.pick(text.bodies.len());
        let body_nodes: Vec<NodeId> = nodes(&graph).filter(|&node| graph.node(node).site().body().index() == body).collect();
        let whole = toy_binding(&mut draw, text.names.len());
        let mut pairs: Vec<(NameId, Term)> = whole.iter().collect();
        for index in (1..pairs.len()).rev() {
            pairs.swap(index, draw.pick(index + 1));
        }
        let mut cuts: Vec<usize> = (0..3).map(|_| draw.pick(pairs.len() + 1)).collect();
        cuts.sort_unstable();
        cuts.push(pairs.len());
        let chain = cuts.iter().map(|&cut| {
            let mut binding = Binding::new();
            for &(name, content) in &pairs[..cut] {
                binding.bind(name, content);
            }
            binding
        });

        let bump = Bump::new();
        let mut widening = Kept::new(&graph, &bump);
        let mut last = Vec::new();
        for binding in chain {
            last.clear();
            for &node in &body_nodes {
                let read = widening.answers(node, &binding, &mut answers);
                let expected = expected(&graph, &mut answers, node, &binding);
                prop_assert_eq!(read, expected.clone());
                last.push(expected);
            }
        }
        let bump = Bump::new();
        let mut at_once = Kept::new(&graph, &bump);
        for (index, &node) in body_nodes.iter().enumerate().rev() {
            let read = at_once.answers(node, &whole, &mut answers);
            prop_assert_eq!(&read, &last[index]);
        }
    }

    /// A passed name binds alike: after evaluating a node, every evaluation the reference reaches,
    /// through calls and through results taken, is filed by its view and answers as the oracle
    /// under its full binding. No more instances rest than the reference reaches.
    #[test]
    fn a_passed_name_binds_alike(raw in raw()) {
        let mut draw = Draw::new(raw);
        let text = toy_text(&mut draw, false);
        let Some((graph, mut answers)) = laid(&text) else { return Ok(()) };
        let binding = toy_binding(&mut draw, text.names.len());
        for node in nodes(&graph) {
            let bump = Bump::new();
            let mut kept = Kept::new(&graph, &bump);
            kept.instance(node, &binding, &mut answers);
            let reached = reference(&graph, &mut answers, node, &binding).reached;
            for each in &reached {
                let found = kept.found(each.node, &each.view, &mut answers);
                prop_assert!(found.is_some(), "{:?} under {:?} is not filed", each.node, each.view);
                let expected = graph.afresh(&Toy, each.node, &each.full, &mut answers);
                prop_assert_eq!(found.expect("filed"), expected);
            }
            prop_assert!(kept.resting() <= reached.len());
        }
    }

    /// Reuse is exact: two bindings of one node get equal handles exactly where the reference's
    /// admissions under their full bindings are equal, and each handle reads as the oracle. The
    /// second binding is drawn to answer alike often: the first with a name no relevant name's
    /// base holds changed, or a bound name moved to another leaf at its depth, or drawn afresh.
    #[test]
    fn reuse_is_exact(raw in raw()) {
        let mut draw = Draw::new(raw);
        let text = toy_text(&mut draw, false);
        let Some((graph, mut answers)) = laid(&text) else { return Ok(()) };
        let bump = Bump::new();
        let mut kept = Kept::new(&graph, &bump);
        let bases = bases(&text);
        for _ in 0..1 + draw.pick(6) {
            let node = NodeId::new(draw.pick(graph.nodes().len()));
            let first = toy_binding(&mut draw, text.names.len());
            let second = match draw.pick(3) {
                0 => {
                    let read: BTreeSet<usize> = graph.relevant(node).iter()
                        .flat_map(|name| bases.names[name.index()].iter().copied())
                        .collect();
                    let unread: Vec<usize> = (0..text.names.len()).filter(|name| !read.contains(name)).collect();
                    let mut second = first.clone();
                    if !unread.is_empty() {
                        second.bind(NameId::new(unread[draw.pick(unread.len())]), atom(9));
                    }
                    second
                }
                1 => {
                    let bound: Vec<(NameId, Term)> = first.iter().collect();
                    let mut second = first.clone();
                    if !bound.is_empty() {
                        let (name, content) = bound[draw.pick(bound.len())];
                        second.bind(name, releafed(content, draw.pick(4) as u8));
                    }
                    second
                }
                _ => toy_binding(&mut draw, text.names.len()),
            };
            let full = |binding: &Binding<Toy>, answers: &mut ToyAnswers| {
                reference(&graph, answers, node, binding).reached.swap_remove(0).full
            };
            let (first_full, second_full) = (full(&first, &mut answers), full(&second, &mut answers));
            let alike = admission(&graph, &mut answers, node, &first_full)
                == admission(&graph, &mut answers, node, &second_full);
            let first_handle = kept.instance(node, &first, &mut answers);
            let second_handle = kept.instance(node, &second, &mut answers);
            prop_assert_eq!(first_handle == second_handle, alike);
            for (words, full) in [(&first_handle, &first_full), (&second_handle, &second_full)] {
                let expected = graph.afresh(&Toy, node, full, &mut answers);
                prop_assert_eq!(kept.read(node, words, &mut answers), expected);
            }
        }
    }

    /// Rebuilt is exact: over the frozen part a load from the root lays, a `Bound` with a drawn
    /// view-table size and a run bump of a drawn budget, its store dropped and its bump reset
    /// where it has passed the budget and at drawn calls, gives the handles a twin keeping
    /// everything gives, and every read through them equals the twin's, just after and at the
    /// end. The run bump never grows past what its first store took.
    #[test]
    fn rebuilt_is_exact(raw in raw()) {
        let mut draw = Draw::new(raw);
        let text = toy_text(&mut draw, false);
        let Some((graph, mut answers)) = laid(&text) else { return Ok(()) };
        let load_bump = Bump::new();
        let mut load = Kept::new(&graph, &load_bump);
        load.instance(graph.root(), &toy_binding(&mut draw, text.names.len()), &mut answers);
        let frozen = load.bound.freeze(load.resting);

        let slots = [Some(0), Some(1), Some(2), None][draw.pick(4)];
        let budget = [Some(0), Some(64), Some(256), Some(1024), None][draw.pick(5)];
        let calls: Vec<(NodeId, Binding<Toy>, bool)> = (0..1 + draw.pick(6))
            .map(|_| {
                let node = NodeId::new(draw.pick(graph.nodes().len()));
                (node, toy_binding_with_answered(&mut draw, &text), draw.chance(1, 3))
            })
            .collect();

        let twin_bump = Bump::new();
        let mut twin = Bound::new(&graph, &Toy, frozen, &twin_bump, None);
        let twin_resting = Resting::new(&twin_bump, None);
        let lend = Bump::new();
        let mut limited = Bound::new(&graph, &Toy, frozen, &lend, slots);
        let mut run_bump = Bump::with_capacity(budget.unwrap_or(0));
        let mut first_size = None;
        let mut made: Vec<(NodeId, Vec<u64>, Vec<Verdict>)> = Vec::new();
        let mut next = 0;
        loop {
            let resting = Resting::new(&run_bump, budget);
            let size = run_bump.allocated_bytes();
            let first_size = *first_size.get_or_insert(size);
            while next < calls.len() {
                let (node, binding, reset) = &calls[next];
                next += 1;
                let limited_words = handle(&mut limited, resting, *node, binding, &mut answers);
                let twin_words = handle(&mut twin, twin_resting, *node, binding, &mut answers);
                prop_assert_eq!(&limited_words, &twin_words);
                let expected = read(&mut twin, twin_resting, *node, &twin_words, &mut answers);
                let got = read(&mut limited, resting, *node, &limited_words, &mut answers);
                prop_assert_eq!(&got, &expected);
                made.push((*node, limited_words, expected));
                if budget.is_some() {
                    prop_assert!(run_bump.allocated_bytes() <= first_size);
                }
                if resting.passed() || *reset {
                    break;
                }
            }
            if next == calls.len() {
                for (node, words, expected) in &made {
                    prop_assert_eq!(&read(&mut limited, resting, *node, words, &mut answers), expected);
                }
                break;
            }
            run_bump.reset();
        }
    }

    /// Finite, on texts whose every name is bound through the text: the load returns, and makes
    /// no more instances than the reference reaches.
    #[test]
    fn finite(raw in raw()) {
        let mut draw = Draw::new(raw);
        let text = toy_text(&mut draw, true);
        let grows = grows_by_terms(&text);
        let Some((graph, mut answers)) = laid(&text) else {
            prop_assert!(grows);
            return Ok(());
        };
        prop_assert!(!grows);
        let none = Binding::new();
        let reached = reference(&graph, &mut answers, graph.root(), &none).reached.len();
        let bump = Bump::new();
        let mut kept = Kept::new(&graph, &bump);
        kept.instance(graph.root(), &none, &mut answers);
        prop_assert!(kept.resting() <= reached);
    }

    /// The growth check refuses exactly what brute-force composition refuses, on every generated
    /// text: open callers and statement names do not bear on growth.
    #[test]
    fn growth_matches_composition(raw in raw()) {
        let text = toy_text(&mut Draw::new(raw), false);
        prop_assert_eq!(laid(&text).is_none(), grows_by_terms(&text));
    }

    /// Start anywhere: an evaluation started afresh at any node and view another start reached
    /// answers as the instance that start reached there. The view leaves out the node's own
    /// answered names, so they are computed again.
    #[test]
    fn start_anywhere(raw in raw()) {
        let mut draw = Draw::new(raw);
        let text = toy_text(&mut draw, false);
        let Some((graph, mut answers)) = laid(&text) else { return Ok(()) };
        let node = NodeId::new(draw.pick(graph.nodes().len()));
        let binding = toy_binding(&mut draw, text.names.len());
        let bump = Bump::new();
        let mut whole = Kept::new(&graph, &bump);
        whole.instance(node, &binding, &mut answers);
        for each in reference(&graph, &mut answers, node, &binding).reached {
            let reached = whole.found(each.node, &each.view, &mut answers).expect("reached");
            let bump = Bump::new();
            let mut fresh = Kept::new(&graph, &bump);
            let started = fresh.answers(each.node, &each.view, &mut answers);
            prop_assert_eq!(&started, &reached);
            prop_assert_eq!(&started, &each.answers);
        }
    }

    /// Knots stay in the text's components: the names and questions of every knot the reference
    /// resolves, from any node, lie in one text component.
    #[test]
    fn knots_stay_in_the_texts_components(raw in raw()) {
        let mut draw = Draw::new(raw);
        let text = toy_text(&mut draw, false);
        let Some((graph, mut answers)) = laid(&text) else { return Ok(()) };
        let binding = toy_binding_with_answered(&mut draw, &text);
        for node in nodes(&graph) {
            for knot in reference(&graph, &mut answers, node, &binding).knots {
                let components: BTreeSet<Option<ComponentId>> = (knot.members.iter())
                    .map(|&member| match member {
                        Unknown::Name(_, name) => graph.name_component(name),
                        Unknown::Question(_, question) => graph.question_component(question),
                    })
                    .collect();
                prop_assert_eq!(components.len(), 1, "{:?} spans {:?}", knot.members, components);
                prop_assert!(!components.contains(&None), "{:?} lies outside", knot.members);
            }
        }
    }

    /// A repeated call walks nothing: repeating any `instance` call gives the same handle, walks
    /// no edge, runs no answer function and rests nothing.
    #[test]
    fn a_repeated_call_walks_nothing(raw in raw()) {
        let mut draw = Draw::new(raw);
        let text = toy_text(&mut draw, false);
        let Some((graph, mut answers)) = laid(&text) else { return Ok(()) };
        let bump = Bump::new();
        let mut kept = Kept::new(&graph, &bump);
        for _ in 0..1 + draw.pick(4) {
            let node = NodeId::new(draw.pick(graph.nodes().len()));
            let binding = toy_binding(&mut draw, text.names.len());
            let first = kept.instance(node, &binding, &mut answers);
            let (walked, laid) = (kept.bound.walked(), kept.resting.laid());
            let mut counting = Counting::new(&mut answers);
            prop_assert_eq!(kept.instance(node, &binding, &mut counting), first);
            prop_assert_eq!(counting.runs.len(), 0);
            prop_assert_eq!((kept.bound.walked(), kept.resting.laid()), (walked, laid));
        }
    }
}

/// `term` with its leaf atom replaced by `leaf`, at the same depth; a term over no atom as it is.
fn releafed(term: Term, leaf: u8) -> Term {
    match node(term) {
        super::toy::Node::List(inner) => list(releafed(inner, leaf)),
        super::toy::Node::Atom(_) => atom(leaf),
        _ => term,
    }
}

/// Against vacuity: of 256 texts drawn with a fixed seed, enough follow an edge, type a name on
/// an arm, through a call and on a site, knot with a question, carry one type left rigid and be
/// typed by a union, knot of another kind holding a union, knot across two bodies, ask `Which` of
/// a rigid, ask of two names at once, answer a borrowed question and follow a call on a site, and
/// the growth check both refuses and lays some. Tune the generator, never the thresholds.
#[test]
fn generators_bite() {
    let mut runner = TestRunner::deterministic();
    let strategy = raw();
    let (mut refused, mut lays) = (0, 0);
    let (mut follows, mut given) = (0, 0);
    let (mut with_question, mut one_type_rigid, mut union_typed, mut union_other) = (0, 0, 0, 0);
    let mut across = 0;
    let mut totals = Reference::default();
    for _ in 0..256 {
        let mut draw = Draw::new(
            strategy
                .new_tree(&mut runner)
                .expect("a raw stream")
                .current(),
        );
        let text = toy_text(&mut draw, false);
        let Some((graph, mut answers)) = laid(&text) else {
            refused += 1;
            continue;
        };
        lays += 1;
        let binding = toy_binding(&mut draw, text.names.len());
        let mut seen = Reference::default();
        let (mut follow, mut question_knot, mut rigid_knot) = (false, false, false);
        let (mut typed_knot, mut other_union_knot, mut across_knot) = (false, false, false);
        for node in nodes(&graph) {
            let up_to = graph.bound_at(node, &binding);
            let run = reference(&graph, &mut answers, node, &up_to);
            follow |= run.reached.len() > 1;
            seen.on_arm += run.on_arm;
            seen.through_call += run.through_call;
            seen.on_site += run.on_site;
            let holds_question = |members: &[Unknown]| {
                (members.iter()).any(|member| matches!(member, Unknown::Question(..)))
            };
            question_knot |= run.knots.iter().any(|knot| holds_question(&knot.members));
            rigid_knot |= run.knots.iter().any(|knot| knot.one_type && !knot.typed);
            typed_knot |= run.knots.iter().any(|knot| knot.typed);
            // A member typed on a site: its one edge, on no arm, carries a term.
            let on_site = |member: &Unknown| match *member {
                Unknown::Name(_, name) => graph.typing(name, None).is_some_and(|edge| {
                    matches!(graph.answers_edge(edge).source(), Source::Site { .. })
                }),
                Unknown::Question(..) => false,
            };
            other_union_knot |=
                (run.knots.iter()).any(|knot| !knot.one_type && knot.members.iter().any(on_site));
            // Each member's body: where its name is bound, or its question owned.
            let body = |member: &Unknown| {
                let node = match *member {
                    Unknown::Name(_, name) => graph.answered_at(name).expect("an answered name"),
                    Unknown::Question(_, question) => graph.question(question).owner(),
                };
                graph.node(node).site().body()
            };
            across_knot |= (run.knots.iter()).any(|knot| {
                let first = body(&knot.members[0]);
                knot.members.iter().any(|member| body(member) != first)
            });
            seen.which_on_rigid += run.which_on_rigid;
            seen.two_names += run.two_names;
            seen.borrowed += run.borrowed;
            seen.site_calls += run.site_calls;
        }
        follows += usize::from(follow);
        with_question += usize::from(question_knot);
        one_type_rigid += usize::from(rigid_knot);
        union_typed += usize::from(typed_knot);
        union_other += usize::from(other_union_knot);
        across += usize::from(across_knot);
        // Drawn last, so the counts above see the draws they always did.
        let typed = toy_binding_with_answered(&mut draw, &text);
        let gives = |node: NodeId| {
            let answered = graph.node(node).answered();
            answered.iter().any(|&name| typed.get(name).is_some())
        };
        given += usize::from(nodes(&graph).any(gives));
        totals.on_arm += usize::from(seen.on_arm > 0);
        totals.through_call += usize::from(seen.through_call > 0);
        totals.on_site += usize::from(seen.on_site > 0);
        totals.which_on_rigid += usize::from(seen.which_on_rigid > 0);
        totals.two_names += usize::from(seen.two_names > 0);
        totals.borrowed += usize::from(seen.borrowed > 0);
        totals.site_calls += usize::from(seen.site_calls > 0);
    }
    let counts = [
        ("follow an edge", follows, 64),
        ("type a name on an arm", totals.on_arm, 16),
        ("type a name through a call", totals.through_call, 16),
        ("type a name on a site", totals.on_site, 16),
        ("knot with a question", with_question, 4),
        ("knot of one type left rigid", one_type_rigid, 4),
        ("knot typed by a union", union_typed, 4),
        ("knot of another kind holding a union", union_other, 4),
        ("knot across two bodies", across, 4),
        ("ask Which of a rigid", totals.which_on_rigid, 16),
        ("ask of two names", totals.two_names, 16),
        ("answer a borrowed question", totals.borrowed, 8),
        ("follow a call on a site", totals.site_calls, 16),
        ("give a node's answered name", given, 16),
    ];
    for (what, count, least) in counts {
        assert!(count >= least, "{count} of 256 texts {what}, under {least}");
    }
    assert!(refused > 0 && lays > 0, "{refused} refused and {lays} laid");
}
