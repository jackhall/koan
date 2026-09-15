//! A callable crossing under a copy: its whole knot re-tied at the destination.
//!
//! `a_copied_knot_outlives_its_home` is on the Miri slate: it is the one path only `function`
//! drives — a knot's run laid down with closure runs and deep copies written into the region while
//! the node run is being filled, read through edges after the region it was copied from is gone.

use std::ptr;

use crate::memory::{CellGraph, ReleaseAbsorption};
use crate::scope::CaptureSlot;
use crate::values::Link;
use crate::values::{Knotted as _, Value, cross};

use super::super::{KValue, KValueFamily, Knotted};
use super::{Fixture, Step, callable, copy, with_fixture};

/// The capture `name` of `callable`'s closure.
fn capture<'graph, 'cell>(
    fixture: &Fixture<'_, 'graph>,
    callable: Knotted<'graph, 'cell>,
    name: &str,
) -> Link<'graph, 'cell, Knotted<'graph, 'cell>> {
    let function = callable.function().expect("a function");
    let name = fixture.name(name);
    let index = function
        .shape()
        .captures()
        .iter()
        .position(|capture| capture.name == name)
        .expect("the body captures the name");
    function.closure().get(CaptureSlot(index as u32))
}

fn captured_value<'graph, 'cell>(
    fixture: &Fixture<'_, 'graph>,
    callable: Knotted<'graph, 'cell>,
    name: &str,
) -> KValue<'graph, 'cell> {
    match capture(fixture, callable, name) {
        Link::Value(value) => value,
        Link::Edge(_) => panic!("`{name}` is captured as a value"),
    }
}

fn captured_sibling<'graph, 'cell>(
    fixture: &Fixture<'_, 'graph>,
    callable: Knotted<'graph, 'cell>,
    name: &str,
) -> Knotted<'graph, 'cell> {
    match capture(fixture, callable, name) {
        Link::Edge(edge) => callable.sibling(edge),
        Link::Value(_) => panic!("`{name}` is captured as an edge"),
    }
}

const KNOT: &str = "\
LET greeting = \"hi\"
LET words = [\"alpha\" \"beta\"]
LET f = (FN :{} -> Str = (greeting words g))
LET g = (FN :{} -> Str = (words f))";

#[test]
fn a_copied_knot_is_the_same_knot_rebuilt() {
    with_fixture(|fixture| {
        let lines = fixture.parse(KNOT);
        let (types, scratch) = (fixture.types, fixture.scratch());
        let mut graph: CellGraph<'_, Step> = CellGraph::new(2, copy);
        let home = graph.create(None, None).unwrap();
        let dest = graph.create(None, None).unwrap();
        graph
            .enter(home, |context| {
                let activation = fixture.run(context.writer(), &lines, dest.into(), &[]);
                let g = callable(fixture, activation, "g");
                let source = context.lift::<KValueFamily>(Value::Knotted(g));
                let crossed = cross(context, dest, &source).unwrap();
                let Value::Knotted(copied) = context.read(&crossed).value() else {
                    panic!("a callable crosses as a callable");
                };
                assert!(!ptr::eq(copied.node(), g.node()), "a copy is a new knot");
                assert_eq!(copied.member().knot().len(), 2);
                assert_eq!(copied.member().index(), g.member().index());
                assert_eq!(copied.ktype(), g.ktype());
                assert_eq!(copied.weight(), g.weight());
                assert!(ptr::eq(
                    copied.function().expect("a function").shape(),
                    g.function().expect("a function").shape()
                ));

                let (f, copied_f) = (
                    captured_sibling(fixture, g, "f"),
                    captured_sibling(fixture, copied, "f"),
                );
                assert!(
                    ptr::eq(
                        copied_f
                            .member()
                            .knot()
                            .member(copied.member().index())
                            .payload(),
                        copied.node()
                    ),
                    "an edge in the copy names a node of the copy"
                );
                for (original, rebuilt) in [(g, copied), (f, copied_f)] {
                    let (Value::List(before), Value::List(after)) = (
                        captured_value(fixture, original, "words"),
                        captured_value(fixture, rebuilt, "words"),
                    ) else {
                        panic!("`words` is captured as a list");
                    };
                    assert!(!ptr::eq(before, after), "a captured value is deep-copied");
                    assert_eq!(
                        Value::List(before).equals(&Value::List(after), types, scratch),
                        Ok(true)
                    );
                }
                assert_eq!(
                    captured_value(fixture, copied_f, "greeting").as_str(),
                    Some("hi")
                );
            })
            .unwrap();
        graph.release(dest, ReleaseAbsorption::IntoHolder).unwrap();
        graph.release(home, ReleaseAbsorption::IntoHolder).unwrap();
        assert!(graph.is_empty());
    });
}

#[test]
fn a_copied_knot_outlives_its_home() {
    with_fixture(|fixture| {
        let lines = fixture.parse(KNOT);
        let mut graph: CellGraph<'_, Step> = CellGraph::new(2, copy);
        let home = graph.create(None, None).unwrap();
        let dest = graph.create(None, None).unwrap();
        let dormant = graph
            .enter(home, |context| {
                let activation = fixture.run(context.writer(), &lines, dest.into(), &[]);
                let f = callable(fixture, activation, "f");
                let source = context.lift::<KValueFamily>(Value::Knotted(f));
                let crossed = cross(context, dest, &source).unwrap();
                context.keep(crossed)
            })
            .unwrap();
        graph.release(home, ReleaseAbsorption::IntoHolder).unwrap();
        graph
            .enter(dest, |context| {
                let carrier = context.redeem(dormant).unwrap();
                let Value::Knotted(f) = context.read(&carrier).value() else {
                    panic!("the kept callable redeems as a callable");
                };
                let g = captured_sibling(fixture, f, "g");
                assert!(ptr::eq(captured_sibling(fixture, g, "f").node(), f.node()));
                assert_eq!(captured_value(fixture, f, "greeting").as_str(), Some("hi"));
                for holder in [f, g] {
                    let words = captured_value(fixture, holder, "words");
                    let words = words.as_list().expect("`words` is a list");
                    assert_eq!(words.get(1).and_then(Value::as_str), Some("beta"));
                }
            })
            .unwrap();
        graph.release(dest, ReleaseAbsorption::IntoHolder).unwrap();
        assert!(graph.is_empty());
    });
}
