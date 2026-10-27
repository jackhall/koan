//! A capture step of a kept path: [`Value::captured`] reads a function's capture slot as an
//! activation of it does, a value word as itself and an edge as the fellow member it names.

use crate::scope::CaptureSlot;
use crate::values::{Knotted as _, Link, Value};

use super::{bound, callable, pin, with_fixture};

const KNOT: &str = "\
LET greeting = \"hi\"
LET f = FN EXPR #(GREET n :Number) -> Str = #(greeting (g n))
LET g = FN EXPR #(TWICE x :Number) -> Str = #(f x)";

#[test]
fn a_capture_step_reads_a_word_as_itself_and_an_edge_as_its_fellow() {
    with_fixture(|fixture| {
        let lines = fixture.parse(KNOT);
        fixture.in_cell(pin, |context| {
            let types = &fixture.tiers.of(context);
            let activation = fixture.run(context.writer(), types, &lines, &[]);
            let f = callable(fixture, activation, "f");
            let closure = f.function().expect("a function").closure();
            for slot in 0..closure.len() as u32 {
                let captured = Value::Knotted(f).captured(slot).expect("a capture slot");
                match closure.get(CaptureSlot(slot)) {
                    Link::Value(word) => assert!(captured.same_storage(&word), "the word itself"),
                    Link::Edge(edge) => assert!(
                        matches!(captured, Value::Knotted(fellow) if fellow == f.sibling(edge)),
                        "the fellow the edge names"
                    ),
                }
            }
            let g = callable(fixture, activation, "g");
            assert!(
                (0..closure.len() as u32).any(|slot| {
                    matches!(Value::Knotted(f).captured(slot), Some(Value::Knotted(fellow)) if fellow == g)
                }),
                "`f` captures `g`, a fellow of its knot"
            );
            assert!(Value::Knotted(f).captured(closure.len() as u32).is_none());
            assert!(bound(fixture, activation, "greeting").captured(0).is_none());
        });
    });
}
