//! The **owner law** of the binding graph each load lays: each site name binds at the site
//! [`Shapes::bind_site`](crate::scope::Shapes::bind_site) gives its unknown, and each question is
//! owned at the latest of the sites, along its chain, where the site names its reads depend on
//! bind — a site name itself, or the names an answered name's deciding question and arms read,
//! transitively — and, for a question inside a quote's code, the code's entry, where an `EVAL`
//! runs it.

use std::collections::HashMap;

use bindgraph::{Graph, NameId, QuestionId, Site, Source};
use proptest::prelude::*;

use crate::build::lay::{GraphFacts, GraphKept};
use crate::program::CellSubstrate;
use crate::scope::{NameAddress, Shapes};
use crate::values::BuildBudget;

use super::generate::resumable;
use super::{Koan, output};

/// The site names `name`'s content depends on, each gathered into `into` once: a site name itself,
/// or an answered name's, through its deciding question's reads.
fn base(
    graph: &Graph<GraphKept>,
    deciding: &HashMap<NameId, QuestionId>,
    name: NameId,
    into: &mut Vec<NameId>,
) {
    if !graph.is_answered(name) {
        if !into.contains(&name) {
            into.push(name);
        }
        return;
    }
    let question = *(deciding.get(&name)).expect("a question reads only names a question decides");
    for read in graph.question(question).reads() {
        base(graph, deciding, *read, into);
    }
}

/// Each drift from the owner law in `source`'s graph: a site name bound elsewhere than its
/// unknown's bind site, or a question owned elsewhere than the latest bind site in its reads'
/// bases and, inside a quote's code, the code's entry.
fn owner_drift(source: &str) -> Vec<String> {
    let mut substrate =
        CellSubstrate::load::<Koan>(source, "<test>", 8, BuildBudget::default(), output())
            .unwrap_or_else(|error| panic!("the program loads: {error}"));
    substrate.with(|running| {
        let laid = running.binding_graph();
        let (graph, facts) = (&laid.graph, laid.facts);
        let shapes = running.program().facts().shapes();
        let mut drift = Vec::new();
        let mut deciding = HashMap::new();
        for edge in graph.answers_edges() {
            match *edge.source() {
                Source::Arm { question, .. } => {
                    deciding.insert(edge.name(), question);
                }
                // An arrival or a callable's result, which only an onward call or another call
                // reads: a question reading one finds no deciding question in `base`.
                Source::Call { .. } | Source::Site { .. } => {}
            }
        }
        let bind_site = |name: NameId| bound_at(facts, shapes, name);
        for (at, question) in graph.questions().iter().enumerate() {
            let mut names = Vec::new();
            for read in question.reads() {
                base(graph, &deciding, *read, &mut names);
            }
            let mut latest: Option<Site> = None;
            for name in names {
                let site = bind_site(name);
                if graph.name_site(name) != site {
                    drift.push(format!(
                        "{name:?} binds at {:?}, its unknown at {site:?}",
                        graph.name_site(name)
                    ));
                }
                if latest.is_none_or(|held| graph.site_lt(held, site)) {
                    latest = Some(site);
                }
            }
            // A question inside a quote's code is asked no earlier than the nearest code's entry.
            let segment = (facts.bodies.iter())
                .position(|held| *held == Some(question.site().body()))
                .map(|at| crate::scope::Segment(at as u32))
                .expect("a graph body is a shape's");
            let code = std::iter::successors(Some(segment), |segment| {
                shapes.parent(*segment).map(|(parent, _)| parent)
            })
            .find(|segment| shapes.shape_at(*segment).kind() == crate::scope::ShapeKind::Code);
            if let Some(code) = code.and_then(|code| facts.bodies[code.index() as usize]) {
                let site = Site::Entry(code);
                if latest.is_none_or(|held| graph.site_lt(held, site)) {
                    latest = Some(site);
                }
            }
            let owner = graph.node(question.owner()).site();
            let expected = latest.unwrap_or(graph.node(graph.root()).site());
            if owner != expected {
                drift.push(format!(
                    "question {at} is owned at {owner:?}, its latest read binds at {expected:?}"
                ));
            }
        }
        drift
    })
}

/// The site the site name `name` binds at: its unknown's bind site.
fn bound_at(facts: &GraphFacts<'_>, shapes: &Shapes<'_>, name: NameId) -> Site {
    let NameAddress::Site(unknown) = facts.name(name).address else {
        panic!("{name:?} is no site name");
    };
    let bound = shapes.bind_site(unknown);
    facts.site(shapes, bound.segment, bound.unit)
}

#[test]
fn report_s_questions_are_owned_where_their_reads_bind() {
    let source = "\
EXPR #(SHOW n :Number) -> Str = #(\"number\")
EXPR #(SHOW s :Str) -> Str = #(\"string\")
EXPR FOR ALL #[Elem] #(REPORT x :Elem code :Any) -> Str = #(
  LET first = (SHOW x)
  LET built = (EVAL code -> Module)
  SHOW (built.sample)
)
PRINT (REPORT 5 #(MODULE made = (LET sample = \"hi\")))";
    assert_eq!(owner_drift(source), Vec::<String>::new());
}

#[test]
fn a_holder_s_questions_are_owned_at_the_holder() {
    // `LET result` is typed through its call, and still owns `SHOW (result.sample)`.
    let source = "\
EXPR #(SHOW n :Number) -> Str = #(\"number\")
EXPR #(SHOW s :Str) -> Str = #(\"string\")
EXPR #(BOX n :Number) -> Module = #(MODULE made = (LET sample = 1))
EXPR #(BOX s :Str) -> Module = #(MODULE made = (LET sample = \"hi\"))
EXPR #(PICK x :Any) -> Module = #(
  BOX x
)
EXPR #(USE) -> Str = #(
  LET result = (PICK 5)
  SHOW (result.sample)
)
PRINT (USE)";
    assert_eq!(owner_drift(source), Vec::<String>::new());
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(crate::tests::case_share(1, 4)))]

    /// Every question of a generated program's graph is owned at the latest bind site in its
    /// reads' bases, or at its quote's code's entry where that is later.
    #[test]
    fn each_question_is_owned_at_the_latest_bind_site_among_its_reads(case in resumable()) {
        let source = case.render();
        let drift = owner_drift(&source);
        prop_assert!(drift.is_empty(), "{:#?} in\n{}", drift, source);
    }
}
