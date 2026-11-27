//! The `perf` counters: what one evaluation asks and runs, what a repeated call and a replay add,
//! and what the search for knots stacks and closes, against the reference.

use bumpstorage::Bump;
use proptest::prelude::*;

use super::case_share;
use super::cases::{Case, bind};
use super::harness::{EMPTY, Kept, handle, read};
use super::laws::laid;
use super::reference::reference;
use super::toy::{Draw, Probe, Toy, atom, list, raw, toy_binding_with_answered, toy_text};
use crate::{Bound, Counts, Handle, NodeId, Resting, Site};

#[test]
fn counts_asks_and_runs() {
    let mut case = Case::new();
    let body = case.text.body(case.text.root());
    let x = case.text.name(Site::Entry(body));
    case.ask(
        Site::Entry(body),
        &[x],
        &[(x, Probe::Leaf), (x, Probe::Depth)],
        0,
    );
    case.ask(Site::Entry(body), &[x], &[(x, Probe::Depth)], 0);
    let (graph, mut answers) = case.lay();
    let entry = graph.entry(body);
    let binding = bind(&[(x, list(atom(2)))]);

    let lend = Bump::new();
    let mut bound = Bound::new(&graph, &Toy, &EMPTY, &lend, None);
    let run = Bump::new();
    let resting = Resting::new(&run, Some(0));
    let words = handle(&mut bound, resting, entry, &binding, &mut answers);
    // The second question's ask is the first's: it reads its field and asks nothing.
    assert_eq!(
        bound.counts(),
        Counts {
            asks: 2,
            runs: 2,
            ..Counts::default()
        }
    );
    handle(&mut bound, resting, entry, &binding, &mut answers);
    assert_eq!(
        bound.counts(),
        Counts {
            asks: 2,
            runs: 2,
            ..Counts::default()
        }
    );
    read(&mut bound, resting, entry, &words, &mut answers);
    assert_eq!(
        bound.counts(),
        Counts {
            asks: 2,
            runs: 4,
            ..Counts::default()
        }
    );
    // One answer replays its question alone.
    let first = graph.node(entry).owns()[0];
    bound.answer(resting, Handle::new(entry, &words), first, &mut answers);
    assert_eq!(
        bound.counts(),
        Counts {
            asks: 2,
            runs: 5,
            ..Counts::default()
        }
    );
}

/// Where every answer rests, one answer and an answered name of a filed view run nothing.
#[test]
fn a_rested_read_runs_nothing() {
    let mut case = Case::new();
    let body = case.text.body(case.text.root());
    let x = case.text.name(Site::Entry(body));
    let tagged = case.text.answered_name();
    let tag = case.ask(Site::Entry(body), &[x], &[(x, Probe::Leaf)], 2);
    case.text.answers(tag, 0, tagged, atom(0));
    case.text.answers(tag, 1, tagged, list(atom(0)));
    let (graph, mut answers) = case.lay();
    let entry = graph.entry(body);
    let binding = bind(&[(x, atom(2))]);

    let bump = Bump::new();
    let mut kept = Kept::new(&graph, &bump);
    let words = kept.instance(entry, &binding, &mut answers);
    let counts = kept.bound.counts();
    kept.bound
        .answer(kept.resting, Handle::new(entry, &words), tag, &mut answers);
    kept.bound
        .content(kept.resting, entry, &binding, tagged, &Toy, &mut answers);
    assert_eq!(kept.bound.counts(), counts);
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: case_share(1, 1),
        ..ProptestConfig::default()
    })]

    /// Over one `instance` call on a fresh `Bound`, the demands put on Tarjan's stack are the
    /// reference's unknowns whose name or question lies in a text component, and the knots closed
    /// are the reference's, carrying one type and of any other kind.
    #[test]
    fn knots_are_counted_as_the_reference_resolves_them(raw in raw()) {
        let mut draw = Draw::new(raw);
        let text = toy_text(&mut draw, false);
        let Some((graph, mut answers)) = laid(&text) else { return Ok(()) };
        let node = NodeId::new(draw.pick(graph.nodes().len()));
        let binding = toy_binding_with_answered(&mut draw, &text);
        let bump = Bump::new();
        let mut kept = Kept::new(&graph, &bump);
        kept.instance(node, &binding, &mut answers);
        let counts = kept.bound.counts();
        let run = reference(&graph, &mut answers, node, &binding);
        let one_type = run.knots.iter().filter(|knot| knot.one_type).count() as u64;
        prop_assert_eq!(counts.stacked, run.in_components as u64);
        prop_assert_eq!(counts.knots_of_one_type, one_type);
        prop_assert_eq!(counts.other_knots, run.knots.len() as u64 - one_type);
    }
}
