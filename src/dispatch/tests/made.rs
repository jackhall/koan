//! The **instance law** of the binding graph each load lays: each instance the load makes, from the
//! root with nothing bound and through every call an answer takes, and every onward call whose
//! holder's names arrive, answers each stored question its
//! node owns as the stored-question law evaluates it under the site names the instance binds —
//! each ask written over those names' contents and asked of the closed door, every decided name
//! its term rebound directly rather than read off its answered name. So the answered names' edges,
//! the contents calls pass and the doors the crate serves agree with the fresh load's evaluation
//! the [stored-question laws](super::stored) check.

use std::collections::{HashMap, HashSet};

use bindgraph::{Binding, Bound, Domain, On, Resting};
use proptest::prelude::*;

use crate::build::lay::{GraphAnswers, GraphQuestion, NameKind, NameVariable};
use crate::memory::Bump;
use crate::program::CellSubstrate;
use crate::scope::{Answer, Asks, DerivedName, NameAddress, Posed, SiteAnswer, Spellings, decide};
use crate::values::tests::reading;
use crate::values::{BuildBudget, Chain};

use super::super::graph::{GraphDomain, Minted, Modules, Named, Naming, ask};
use super::generate::resumable;
use super::{Koan, output};

/// How many instances the law makes of one program at most: a recursion the load follows files
/// each view once, so this only bounds a test's work.
const INSTANCES: usize = 256;

/// Each ask with the instance's site names written in, asked of the closed door.
struct Direct<'q, 'n, 'x, 'tier, 'pair, 'reg, 'door> {
    types: &'x Chain<'tier, 'pair, 'reg, 'door>,
    scratch: &'x Bump,
    naming: Naming<'n>,
    modules: Modules<'x, 'q>,
    asks: &'q [Posed<'q>],
    names: &'x [Named<'q>],
}

impl Asks for Direct<'_, '_, '_, '_, '_, '_, '_> {
    fn ask(&mut self, index: u16) -> Option<Answer> {
        let question = &self.asks[usize::from(index)].question;
        let (types, scratch) = (self.types, self.scratch);
        ask(
            types,
            scratch,
            self.naming,
            Some(self.modules),
            question,
            self.names,
        )
    }
}

/// Each drift from the instance law in `source`'s graph — a stored question an instance the load
/// makes answers otherwise than the stored-question law under the instance's site names — beside
/// how many answers the law compared, and how many holders it entered through an onward call.
fn instance_drift(source: &str) -> (Vec<String>, usize, usize) {
    let mut substrate =
        CellSubstrate::load::<Koan>(source, "<test>", 8, BuildBudget::default(), output())
            .unwrap_or_else(|error| panic!("the program loads: {error}"));
    substrate.with(|running| {
        let laid = running.binding_graph();
        let (graph, facts) = (&laid.graph, laid.facts);
        let load = running.program().facts();
        let shapes = load.shapes();
        let bump = Bump::new();
        let types = reading(running.program().tiers(), &bump);
        let minted = Minted::new(&bump);
        let names = |id| load.derived_name(id).or_else(|| minted.get(id));
        let keep = |name: DerivedName<'_>| minted.keep(name);
        let naming = Naming {
            names: &names,
            spellings: Spellings::new(running.symbols()),
            laid: &keep,
        };
        let domain = GraphDomain {
            facts,
            load,
            types: &types,
            scratch: &bump,
            naming,
            noted: None,
        };
        let lend = Bump::new();
        let resting = Resting::new(&lend, None);
        let mut instances = Bound::new(graph, facts, laid.frozen, &lend, None);
        let mut answers = GraphAnswers { facts };
        let (mut drift, mut compared, mut onward) = (Vec::new(), 0, 0);
        let mut made = HashSet::new();
        let mut queue = vec![(graph.root(), Binding::new(), HashMap::new())];
        while let Some((node, binding, mut known)) = queue.pop() {
            if made.len() == INSTANCES || !made.insert((node, binding.clone())) {
                continue;
            }
            let mut words = vec![0u64; graph.node(node).handle_width()];
            let handle =
                instances.instance(resting, node, &binding, &domain, &mut answers, &mut words);
            // The site names the instance binds, each in the variable a question reads it by: a
            // type name by its level; a module binder's names by their address alone.
            let mut site = Vec::new();
            for (name, content) in binding.iter() {
                let held = facts.name(name);
                let NameAddress::Site(unknown) = held.address else {
                    continue;
                };
                let variable = match held.kind {
                    NameKind::Type => shapes.level(unknown.segment, unknown.at),
                    NameKind::Carried | NameKind::Content => None,
                };
                site.push(Named {
                    variable: variable.map(NameVariable::Level),
                    address: held.address,
                    upper: held.upper,
                    content: Some(content),
                });
            }
            for question in graph.node(node).owns() {
                let answer = instances.answer(resting, handle, *question, &mut answers);
                known.insert(*question, answer);
                let GraphQuestion::Site { question: stored, .. } = *facts.question(*question)
                else {
                    continue;
                };
                let modules = Modules {
                    facts,
                    load,
                    segment: stored.segment,
                };
                let mut direct = Direct {
                    types: &types,
                    scratch: &bump,
                    naming,
                    modules,
                    asks: stored.asks,
                    names: &site,
                };
                let expected = decide(stored, &mut direct);
                compared += 1;
                if answer != expected {
                    drift.push(format!(
                        "{node:?} {:?} at {:?}: {answer:?}, the stored law {expected:?} under {binding:?}",
                        stored.rule, stored.site
                    ));
                }
            }
            let mut full = binding.clone();
            for name in graph.node(node).answered() {
                let content =
                    instances.content(resting, node, &binding, *name, &domain, &mut answers);
                full.bind(*name, content);
            }
            for edge in graph.node(node).follows() {
                let taken = match graph.edge(*edge).on() {
                    On::Site(_) => true,
                    On::Arm { question, arm } => known
                        .get(&question)
                        .and_then(|answer: &SiteAnswer| answer.arm())
                        .is_some_and(|taken| u32::from(taken) == arm),
                };
                if !taken {
                    continue;
                }
                let passed = graph.passed(&domain, *edge, &full);
                // An onward call enters the holder's own node, where every name it binds and
                // reads arrives; a call, its callee's entry.
                let target = match graph.edge(*edge).onward() {
                    Some(site) => {
                        let target = graph.node_at(site).expect("an onward call's target");
                        let relevant = graph.relevant(target);
                        let arrived = (graph.node(target).names().iter())
                            .filter(|name| relevant.binary_search(name).is_ok())
                            .all(|&name| {
                                (passed.get(name))
                                    .is_some_and(|content| domain.arrives(name, content))
                            });
                        if !arrived {
                            continue;
                        }
                        onward += 1;
                        target
                    }
                    None => graph.entry(graph.edge(*edge).callee()),
                };
                queue.push((target, passed, known.clone()));
            }
        }
        (drift, compared, onward)
    })
}

/// The questions the load's instances own, by node: none drift for `KEEP`'s instance at the list
/// `WRAP 5` passes, nor for `REPORT`'s at `5`, nor for the holders the load instances through
/// onward calls.
#[test]
fn the_load_s_instances_answer_as_the_stored_law_does() {
    let keep = "\
EXPR #(SHOW n :Number) -> Str = #(\"number\")
EXPR #(SHOW l :(LIST OF Any)) -> Str = #(\"list\")
EXPR FOR ALL #[Elem] #(KEEP x :Elem) -> Elem = #((PRINT (SHOW x)) (x))
EXPR FOR ALL #[Item] #(WRAP y :Item) -> :(LIST OF Item) = #(KEEP [y])
PRINT (WRAP 5)";
    let report = "\
EXPR #(SHOW n :Number) -> Str = #(\"number\")
EXPR #(SHOW s :Str) -> Str = #(\"string\")
EXPR FOR ALL #[Elem] #(REPORT x :Elem code :Any) -> Str = #(
  LET first = (SHOW x)
  LET built = (EVAL code -> Module)
  SHOW (built.sample)
)
PRINT (REPORT 5 #(MODULE made = (LET sample = \"hi\")))";
    let holders = "\
EXPR #(SHOW n :Number) -> Str = #(\"number\")
EXPR #(SHOW s :Str) -> Str = #(\"string\")
EXPR #(BOX n :Number) -> Module = #(MODULE made = (LET sample = 1))
EXPR #(BOX s :Str) -> Module = #(MODULE made = (LET sample = \"hi\"))
EXPR FOR ALL #[Elem] #(SPIN x :Elem n :Number) -> Module = #(
  MATCH n WITH #{0: (BOX x), Number: (SPIN x (n - 1))}
)
EXPR #(PICK x :Any) -> Module = #(
  BOX x
)
EXPR #(MID z :Any) -> Module = #(
  LET first = (PICK z)
  BOX (first.sample)
)
EXPR #(USE) -> Str = #(
  LET spun = (SPIN \"a\" 3)
  LET picked = (MID 5)
  LET shown = (SHOW (spun.sample))
  SHOW (picked.sample)
)
PRINT (USE)";
    for (source, holds) in [(keep, false), (report, false), (holders, true)] {
        let (drift, compared, onward) = instance_drift(source);
        assert_eq!(onward > 0, holds, "{source}");
        assert_eq!(drift, Vec::<String>::new(), "{source}");
        assert!(
            compared > 0,
            "the load's instances own a stored question of\n{source}"
        );
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(crate::tests::case_share(1, 4)))]

    /// Every instance the load makes of a generated program answers its node's stored questions
    /// as the stored-question law does under the site names it binds.
    #[test]
    fn each_load_made_instance_answers_as_the_stored_law(case in resumable()) {
        let source = case.render();
        let (drift, ..) = instance_drift(&source);
        prop_assert!(drift.is_empty(), "{:#?} in\n{}", drift, source);
    }
}
