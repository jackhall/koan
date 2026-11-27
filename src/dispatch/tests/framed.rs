//! The laws of the instances a run cell's frames make, over generated programs and bindings: each
//! bind site of `SUBJECT`'s body, and of each body around it, made through the door a frame makes
//! its instances by ([`Door::framed`]), its binding laid as a frame lays its slots, a module by
//! its content's digest.
//!
//! - An instance a frame makes answers each question it owns as the stored-question law does
//!   under the binding, which [the stored laws](super::stored) check against a fresh load with the
//!   binding declared, and [the instance law](super::made) against the instances the load makes.
//!   So does each instance a run of the program makes, `SUBJECT` called with values: the frame
//!   gathers its binding off its own slots, captures and held slots.
//! - **Confluent**: making the instances under a widening chain of bindings, its names in any
//!   order, reads as a fresh store making them under the whole at once.

use std::cell::RefCell;

use bindgraph::{NameId, NodeId};
use proptest::prelude::*;

use crate::build::lay::GraphQuestion;
use crate::scope::{Bound, NameAddress, SiteAnswer, Unknown};
use crate::type_lattice::KType;

use super::generate::{Resumable, called, resumable};
use super::stored::{decided_at, written};
use super::under::laws::staged;
use super::under::{Door, opened, ran, subject};

/// An instance a run made, as the bind site hands it: its node, its handle's words, and each type
/// name it binds beside its type.
type Made = (NodeId, Vec<u64>, Vec<(NameId, KType)>);

thread_local! {
    /// The instances the run under test made, in order.
    static MADE: RefCell<Vec<Made>> = const { RefCell::new(Vec::new()) };
}

/// Each drift of the instances a run of `case`, called as `call`, makes of `SUBJECT`'s bind sites
/// from the stored-question law under each one's binding, beside how many answers the law
/// compared: `None` where the program does not load.
fn run_drift(case: &Resumable, call: &str) -> Option<(Vec<String>, usize)> {
    use crate::program::INSTANCED;
    let source = format!("{}{call}", case.render());
    MADE.with(|made| made.borrow_mut().clear());
    INSTANCED.with(|hook| {
        hook.set(Some(|node, words, typed| {
            MADE.with(|made| {
                made.borrow_mut()
                    .push((node, words.to_vec(), typed.to_vec()))
            })
        }))
    });
    let drift = ran(&source, |door| {
        let made = MADE.with(|made| std::mem::take(&mut *made.borrow_mut()));
        let instances = door.instances();
        let laid = instances.binding;
        let shapes = door.load().shapes();
        let body = subject(door.root());
        let own = laid.facts.bodies[body.segment().index() as usize];
        let (mut drift, mut compared) = (Vec::new(), 0);
        for (node, words, typed) in made {
            if Some(laid.graph.node(node).site().body()) != own {
                continue;
            }
            // Each type name the instance binds, by the level a stored question reads it at.
            let levels: Vec<(usize, KType)> = (typed.iter())
                .filter_map(|(name, ty)| {
                    let NameAddress::Site(unknown) = laid.facts.name(*name).address else {
                        return None;
                    };
                    Some((shapes.level(unknown.segment, unknown.at)?, *ty))
                })
                .collect();
            for question in laid.graph.node(node).owns() {
                let GraphQuestion::Site {
                    question: stored, ..
                } = *laid.facts.question(*question)
                else {
                    continue;
                };
                if shapes.owner(stored.segment) != body.segment() {
                    continue;
                }
                let Some(expected) = decided_at(door, &levels, stored) else {
                    continue;
                };
                compared += 1;
                let answer = instances.answer_of(node, &words, *question);
                if answer != expected {
                    drift.push(format!(
                        "{:?} at {:?} of {:?}: {answer:?}, the stored law {expected:?} under {levels:?}",
                        stored.rule, stored.site, stored.segment
                    ));
                }
            }
        }
        (drift, compared)
    });
    INSTANCED.with(|hook| hook.set(None));
    drift
}

/// What a frame of `SUBJECT` reads each of the load's own questions as, in their order, once its
/// slots bind `staged`: the answer of the instance a bind site made, or `None` where no instance
/// owns the question and the load's record stands. A frame reads only its own body's questions,
/// those an encloser's bind site owns among them through the words the encloser held; `None` for
/// any other body's.
fn framed<'d>(door: &Door<'d, '_>, staged: &[(Unknown, Bound<'d>)]) -> Vec<Option<SiteAnswer>> {
    let body = subject(door.root());
    let (facts, _) = door.framed(body, staged);
    let shapes = door.load().shapes();
    let read = |segment| shapes.owner(segment) == body.segment();
    (door.load().own_questions())
        .map(|question| {
            let shape = shapes.shape_at(question.segment);
            (read(question.segment))
                .then(|| facts.site_answer(shape, question.site, question.rule))
                .flatten()
        })
        .collect()
}

/// The order `seed` draws over `count` names: a shuffle by a split-mix stream.
fn order(seed: u64, count: usize) -> Vec<usize> {
    let mut state = seed;
    let mut next = || {
        state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut mixed = state;
        mixed = (mixed ^ mixed >> 30).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        mixed = (mixed ^ mixed >> 27).wrapping_mul(0x94d0_49bb_1331_11eb);
        mixed ^ mixed >> 31
    };
    let mut order: Vec<usize> = (0..count).collect();
    for at in (1..count).rev() {
        order.swap(at, (next() % (at as u64 + 1)) as usize);
    }
    order
}

/// The chain of bindings `staged` widens through, its entries in `order`: nothing bound, then one
/// more name at each step, to the whole.
fn chain<'d>(staged: &[(Unknown, Bound<'d>)], order: &[usize]) -> Vec<Vec<(Unknown, Bound<'d>)>> {
    (0..=order.len())
        .map(|cut| order[..cut].iter().map(|at| staged[*at]).collect())
        .collect()
}

/// Each drift of `case`'s frame from the stored-question law under its binding, beside how many
/// answers the law compared.
fn drift(case: &Resumable) -> (Vec<String>, usize) {
    opened(&case.render(), |door| {
        let framed = framed(door, &staged(door, &case.bound, true));
        let written = written(door, &case.bound);
        let mut drift = Vec::new();
        let mut compared = 0;
        let questions = door.load().own_questions();
        for ((answer, expected), question) in framed.iter().zip(&written).zip(questions) {
            let (Some(answer), Some(expected)) = (answer, expected) else {
                continue;
            };
            compared += 1;
            if answer != expected {
                drift.push(format!(
                    "{:?} at {:?} of {:?}: {answer:?}, the stored law {expected:?}",
                    question.rule, question.site, question.segment
                ));
            }
        }
        (drift, compared)
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(crate::tests::case_share(1, 4)))]

    /// Each question an instance a run of `SUBJECT` makes owns answers as the stored-question law
    /// decides it under the binding the frame gathered.
    #[test]
    fn a_run_s_instance_answers_as_the_stored_law((case, call) in called()) {
        // A program refused at the load runs no frame: the stored laws hold its refusal.
        let Some((drift, _)) = run_drift(&case, &call) else {
            return Ok(());
        };
        prop_assert!(drift.is_empty(), "{:#?} in\n{}{}", drift, case.render(), call);
    }

    /// Each question an instance a frame makes owns answers as the stored-question law decides it
    /// under the case's binding.
    #[test]
    fn a_frame_s_instance_answers_as_the_stored_law(case in resumable()) {
        let (drift, _) = drift(&case);
        prop_assert!(drift.is_empty(), "{:#?} in\n{}\nunder {:?}", drift, case.render(), case.bound);
    }

    /// The instances a store makes along a chain of bindings, one more name bound at a time in
    /// any order, read at each step as those a fresh store makes under that step's binding at
    /// once: what one store filed before a step changes none of the step's answers.
    #[test]
    fn a_frame_s_answers_are_one_in_whatever_order_its_binding_grows(case in resumable(), seed in any::<u64>()) {
        let source = case.render();
        let count = opened(&source, |door| staged(door, &case.bound, true).len());
        let order = order(seed, count);
        let chained = opened(&source, |door| {
            let staged = staged(door, &case.bound, true);
            (chain(&staged, &order).iter()).map(|step| framed(door, step)).collect::<Vec<_>>()
        });
        for (cut, chained) in chained.iter().enumerate() {
            let at_once = opened(&source, |door| {
                let staged = staged(door, &case.bound, true);
                framed(door, &chain(&staged, &order)[cut])
            });
            prop_assert_eq!(chained, &at_once, "step {} in\n{}\nunder {:?}", cut, source, case.bound);
        }
    }
}

/// The frame law bites: on a fixed seed, one of 128 cases compares an instance's answer.
#[test]
fn the_frame_law_compares_an_instance_s_answers() {
    use proptest::strategy::ValueTree;
    use proptest::test_runner::{RngAlgorithm, TestRng, TestRunner};
    let mut runner = TestRunner::new_with_rng(
        ProptestConfig::default(),
        TestRng::deterministic_rng(RngAlgorithm::ChaCha),
    );
    let strategy = resumable();
    let compared = (0..128).any(|_| {
        let case = strategy.new_tree(&mut runner).expect("a case").current();
        drift(&case).1 > 0
    });
    assert!(compared, "no case compares an instance's answer");
}

/// The run law bites: on a fixed seed, one of 128 cases loads, runs and compares an instance's
/// answer.
#[test]
fn the_run_law_compares_an_instance_s_answers() {
    use proptest::strategy::ValueTree;
    use proptest::test_runner::{RngAlgorithm, TestRng, TestRunner};
    let mut runner = TestRunner::new_with_rng(
        ProptestConfig::default(),
        TestRng::deterministic_rng(RngAlgorithm::ChaCha),
    );
    let strategy = called();
    let compared = (0..128).any(|_| {
        let (case, call) = strategy.new_tree(&mut runner).expect("a case").current();
        run_drift(&case, &call).is_some_and(|(_, compared)| compared > 0)
    });
    assert!(compared, "no run compares an instance's answer");
}

/// A binding giving an unbounded `FOR ALL` name a signature type reads as a fresh load with it
/// declared: the load refuses every read of the binder as a module, which only some bindings
/// would allow, and a body that loads reads the binder alike under both.
#[test]
fn a_for_all_name_bound_to_a_signature_reads_as_a_fresh_load_does() {
    let prelude = "\
SIG Counter FOR ALL #[Carrier] = #[(VAL zero :Carrier)]
MODULE ints = ((LET Carrier = Number) (LET zero = 0))
EXPR #(TAKE c :Counter) -> Str = #(\"counter\")
EXPR #(TAKE a :Any) -> Str = #(\"any\")
";
    let generic = |body: &str| {
        format!(
            "{prelude}EXPR FOR ALL #[Elt] #(SUBJECT x :Elt) -> Any = #(\n  {body}\n)\n\
             PRINT (SUBJECT ints)"
        )
    };
    let declared = |body: &str| {
        format!(
            "{prelude}LET Elt = :Module\nEXPR #(SUBJECT x :Elt) -> Any = #(\n  {body}\n)\n\
             PRINT (SUBJECT ints)"
        )
    };
    let refused = super::run(&generic("x.zero"));
    assert!(refused.contains("Elt has no field zero"), "{refused}");
    assert_eq!(super::run(&declared("x.zero")), "0");
    assert_eq!(super::run(&generic("TAKE x")), "counter");
    assert_eq!(super::run(&declared("TAKE x")), "counter");
}
