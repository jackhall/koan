//! A knot member crossing under a copy: its whole knot re-tied at the destination.
//!
//! `a_copied_knot_outlives_its_home`, `a_copied_ring_outlives_its_home`,
//! `a_copied_module_outlives_its_home` and `a_copied_barrier_outlives_its_home` are on the Miri
//! slate:
//! they are the paths only `knot` drives — a knot's run laid down with closure runs, data nodes
//! and deep copies written into the region while the node run is being filled, read through edges
//! after the region it was copied from is gone.

use std::ptr;

use crate::memory::{CellGraph, ReleaseAbsorption};
use crate::scope::{CaptureSlot, Slot};
use crate::values::{Circular, Link};
use crate::values::{Knotted as _, Value, cross};

use super::super::{Coerced, KValue, KValueFamily, Knotted};
use super::{Fixture, Step, bound, callable, circular, copy, declared, follow, with_fixture};

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
LET g = (FN FOR ALL (Elt) :{x :Elt} -> Elt = (words f x))";

#[test]
fn a_copied_knot_is_the_same_knot_rebuilt() {
    with_fixture(|fixture| {
        let lines = fixture.parse(KNOT);
        let (types, scratch) = (fixture.types, fixture.scratch());
        let mut graph: CellGraph<'_, Step> = CellGraph::new(2, copy);
        let home = graph.create(None).unwrap();
        let dest = graph.create(None).unwrap();
        graph
            .enter(home, |context| {
                let activation = fixture.run(context.writer(), &lines, &[]);
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
                // The quantifier map is a run in the source region, so the copy re-homes it: a
                // different address holding the same entries.
                let map = g.function().expect("a function").quantifier_map();
                let copied_map = copied.function().expect("a function").quantifier_map();
                assert_eq!(map, [Some(0)]);
                assert_eq!(copied_map, map);
                assert!(!ptr::eq(copied_map, map), "the run is re-homed, not shared");

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
        let home = graph.create(None).unwrap();
        let dest = graph.create(None).unwrap();
        let dormant = graph
            .enter(home, |context| {
                let activation = fixture.run(context.writer(), &lines, &[]);
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

const RING: &str = "\
NEWTYPE Ring = :{next :Ring}
LET a = (Ring {next = b})
LET b = (Ring {next = a})
LET f = (FN :{} -> Any = (a))";

/// The member `next` of a ring node names: its tagged payload's record node, then that record's
/// `next` field.
fn successor<'graph, 'cell>(
    fixture: &Fixture<'_, 'graph>,
    member: Knotted<'graph, 'cell>,
) -> Knotted<'graph, 'cell> {
    let (_, Circular::Tagged(tagged)) = circular(Value::Knotted(member)) else {
        panic!("a ring member is tagged");
    };
    let record = follow(member, *tagged.payload());
    let (_, Circular::Record(fields)) = circular(Value::Knotted(record)) else {
        panic!("its payload is a record node");
    };
    follow(
        record,
        *fields.field(fixture.name("next").symbol()).unwrap(),
    )
}

#[test]
fn a_copied_ring_is_the_same_graph_rebuilt() {
    with_fixture(|fixture| {
        let lines = fixture.parse(RING);
        let (types, scratch) = (fixture.types, fixture.scratch());
        let mut graph: CellGraph<'_, Step> = CellGraph::new(2, copy);
        let home = graph.create(None).unwrap();
        let dest = graph.create(None).unwrap();
        graph
            .enter(home, |context| {
                let activation = fixture.run(context.writer(), &lines, &[]);
                let ring = declared(fixture, activation, "Ring");
                let f = callable(fixture, activation, "f");
                let source = context.lift::<KValueFamily>(Value::Knotted(f));
                let crossed = cross(context, dest, &source).unwrap();
                let Value::Knotted(copied_f) = context.read(&crossed).value() else {
                    panic!("a callable crosses as a callable");
                };
                let (Value::Knotted(a), Value::Knotted(copied_a)) = (
                    captured_value(fixture, f, "a"),
                    captured_value(fixture, copied_f, "a"),
                ) else {
                    panic!("`a` is captured as a knot member");
                };
                assert!(
                    !(copied_a.member().knot() == a.member().knot()),
                    "a copied ring is a new knot"
                );
                assert_eq!(copied_a.member().knot().len(), a.member().knot().len());
                assert_eq!(copied_a.member().index(), a.member().index());
                assert_eq!(copied_a.ktype(), ring);

                let (Some((_, Circular::Tagged(before))), Some((_, Circular::Tagged(after)))) = (
                    Value::Knotted(a).as_circular(),
                    Value::Knotted(copied_a).as_circular(),
                ) else {
                    panic!("a ring member is a tagged node");
                };
                let (Link::Edge(source_edge), Link::Edge(copied_edge)) =
                    (*before.payload(), *after.payload())
                else {
                    panic!("a ring member's payload is an edge");
                };
                assert_eq!(source_edge, copied_edge, "an edge is carried verbatim");
                let copied_b = successor(fixture, copied_a);
                assert!(copied_b.member().knot() == copied_a.member().knot());
                assert!(successor(fixture, copied_b) == copied_a);
                assert_eq!(
                    Value::Knotted(a).equals(&Value::Knotted(copied_a), types, scratch),
                    Ok(true)
                );
            })
            .unwrap();
        graph.release(dest, ReleaseAbsorption::IntoHolder).unwrap();
        graph.release(home, ReleaseAbsorption::IntoHolder).unwrap();
        assert!(graph.is_empty());
    });
}

#[test]
fn a_copied_ring_outlives_its_home() {
    with_fixture(|fixture| {
        let lines = fixture.parse(
            "NEWTYPE Tag = :{next :Tag, name :Str, items :(LIST OF Tag)}\n\
             LET a = (Tag {next = a name = \"ring\" items = [a]})",
        );
        let mut graph: CellGraph<'_, Step> = CellGraph::new(2, copy);
        let home = graph.create(None).unwrap();
        let dest = graph.create(None).unwrap();
        let dormant = graph
            .enter(home, |context| {
                let activation = fixture.run(context.writer(), &lines, &[]);
                let a = super::bound(fixture, activation, "a");
                let source = context.lift::<KValueFamily>(a);
                let crossed = cross(context, dest, &source).unwrap();
                context.keep(crossed)
            })
            .unwrap();
        graph.release(home, ReleaseAbsorption::IntoHolder).unwrap();
        graph
            .enter(dest, |context| {
                let carrier = context.redeem(dormant).unwrap();
                let (a, Circular::Tagged(tagged)) = circular(context.read(&carrier).value()) else {
                    panic!("the kept ring redeems as a tagged node");
                };
                assert_eq!(a.member().knot().len(), 3);
                let record = follow(a, *tagged.payload());
                let (_, Circular::Record(fields)) = circular(Value::Knotted(record)) else {
                    panic!("the payload is a record node");
                };
                let field = |name: &str| *fields.field(fixture.name(name).symbol()).unwrap();
                assert!(follow(record, field("next")) == a);
                let Link::Value(name) = field("name") else {
                    panic!("`name` is a value cell");
                };
                assert_eq!(name.as_str(), Some("ring"));
                let items = follow(record, field("items"));
                let (_, Circular::List(list)) = circular(Value::Knotted(items)) else {
                    panic!("`items` is an anonymous list node");
                };
                assert!(follow(items, list.cells()[0]) == a);
            })
            .unwrap();
        graph.release(dest, ReleaseAbsorption::IntoHolder).unwrap();
        assert!(graph.is_empty());
    });
}

const MODULE: &str = "\
LET greeting = \"hi\"
MODULE m = (\
(LET words = [\"alpha\" \"beta\"]) \
(LET f = (FN :{} -> Str = (greeting g))) \
(LET g = (FN :{} -> Str = (f))) \
(NEWTYPE Dist = Number))";

/// The slot each named member of `m`'s body takes — the body shape lives in program storage, so
/// one reading serves the source module and its copy in another region alike.
fn member_slots<const N: usize>(
    fixture: &Fixture<'_, '_>,
    activation: &super::super::KActivation<'_, '_>,
    names: [&str; N],
) -> [Slot; N] {
    let shape = activation.shape();
    let (binder, _) = shape.slot(fixture.name("m")).expect("`m` is declared");
    let body = shape
        .births(binder)
        .expect("a module binder births its body");
    names.map(|name| body.slot(fixture.name(name)).expect("a declared member").0)
}

/// The member of `module` at `slot`.
fn member<'graph, 'cell>(module: Knotted<'graph, 'cell>, slot: Slot) -> KValue<'graph, 'cell> {
    module.module().expect("a module node").members()[slot.index()]
}

#[test]
fn a_copied_module_is_the_same_members_rebuilt() {
    with_fixture(|fixture| {
        let lines = fixture.parse(MODULE);
        let (types, scratch) = (fixture.types, fixture.scratch());
        let mut graph: CellGraph<'_, Step> = CellGraph::new(2, copy);
        let home = graph.create(None).unwrap();
        let dest = graph.create(None).unwrap();
        graph
            .enter(home, |context| {
                let activation = fixture.run(context.writer(), &lines, &[]);
                let slots = member_slots(fixture, activation, ["words", "f", "Dist"]);
                let m = bound(fixture, activation, "m")
                    .as_module()
                    .expect("`m` is a module");
                let source = context.lift::<KValueFamily>(Value::Knotted(m));
                let crossed = cross(context, dest, &source).unwrap();
                let Value::Knotted(copied) = context.read(&crossed).value() else {
                    panic!("a module crosses as a module");
                };
                assert!(!ptr::eq(copied.node(), m.node()), "a copy is a new knot");
                assert_eq!(copied.member().knot().len(), 1);
                assert_eq!(copied.ktype(), m.ktype());
                assert_eq!(copied.weight(), m.weight());
                assert_eq!(copied.module().expect("a module node").members().len(), 4);

                // Each member is rebuilt: a container deep-copied, a type value carried, and a
                // member that is itself a knot member bringing its whole knot with it.
                let (before, after) = (member(m, slots[0]), member(copied, slots[0]));
                assert!(!ptr::eq(
                    before.as_list().expect("a list member"),
                    after.as_list().expect("a list member"),
                ));
                assert_eq!(before.equals(&after, types, scratch), Ok(true));
                assert_eq!(
                    member(copied, slots[2]).ktype(),
                    member(m, slots[2]).ktype(),
                );
                let f = member(copied, slots[1])
                    .as_callable()
                    .expect("a callable member");
                assert_eq!(
                    f.member().knot().len(),
                    2,
                    "the function knot arrives whole"
                );
                assert!(captured_sibling(fixture, f, "g").function().is_some());
                assert_eq!(captured_value(fixture, f, "greeting").as_str(), Some("hi"));
            })
            .unwrap();
        graph.release(dest, ReleaseAbsorption::IntoHolder).unwrap();
        graph.release(home, ReleaseAbsorption::IntoHolder).unwrap();
        assert!(graph.is_empty());
    });
}

#[test]
fn a_copied_module_outlives_its_home() {
    with_fixture(|fixture| {
        let lines = fixture.parse(MODULE);
        let mut graph: CellGraph<'_, Step> = CellGraph::new(2, copy);
        let home = graph.create(None).unwrap();
        let dest = graph.create(None).unwrap();
        let (dormant, slots) = graph
            .enter(home, |context| {
                let activation = fixture.run(context.writer(), &lines, &[]);
                let slots = member_slots(fixture, activation, ["words", "f", "g"]);
                let m = bound(fixture, activation, "m")
                    .as_module()
                    .expect("`m` is a module");
                let source = context.lift::<KValueFamily>(Value::Knotted(m));
                let crossed = cross(context, dest, &source).unwrap();
                (context.keep(crossed), slots)
            })
            .unwrap();
        graph.release(home, ReleaseAbsorption::IntoHolder).unwrap();
        graph
            .enter(dest, |context| {
                let carrier = context.redeem(dormant).unwrap();
                let Value::Knotted(m) = context.read(&carrier).value() else {
                    panic!("the kept module redeems as a module");
                };
                let words = member(m, slots[0]).as_list().expect("a list member");
                assert_eq!(words.get(1).and_then(Value::as_str), Some("beta"));
                let f = member(m, slots[1])
                    .as_callable()
                    .expect("a callable member");
                let g = member(m, slots[2])
                    .as_callable()
                    .expect("a callable member");
                assert_eq!(captured_value(fixture, f, "greeting").as_str(), Some("hi"));
                // Each member was rebuilt through the one crossing, bringing its whole knot with
                // it, so the two members of the body's function knot arrive as two copies of it —
                // each internally consistent, neither naming the other's nodes.
                let (fs_g, gs_f) = (
                    captured_sibling(fixture, f, "g"),
                    captured_sibling(fixture, g, "f"),
                );
                assert!(!ptr::eq(fs_g.node(), g.node()));
                assert!(!ptr::eq(gs_f.node(), f.node()));
                assert!(ptr::eq(
                    captured_sibling(fixture, fs_g, "f").node(),
                    f.node()
                ));
                assert!(ptr::eq(
                    captured_sibling(fixture, gs_f, "g").node(),
                    g.node()
                ));
            })
            .unwrap();
        graph.release(dest, ReleaseAbsorption::IntoHolder).unwrap();
        assert!(graph.is_empty());
    });
}

#[test]
fn a_copied_barrier_outlives_its_home() {
    with_fixture(|fixture| {
        let lines = fixture.parse("LET greeting = \"hi\"\nLET f = (FN :{} -> Str = (greeting))");
        let mut graph: CellGraph<'_, Step> = CellGraph::new(2, copy);
        let home = graph.create(None).unwrap();
        let dest = graph.create(None).unwrap();
        let (dormant, ktype) = graph
            .enter(home, |context| {
                let writer = context.writer();
                let activation = fixture.run(writer, &lines, &[]);
                let f = callable(fixture, activation, "f");
                // The barrier's types are this item's only fiction: a real view substitutes, which
                // is the module layer's work. What is pinned here is that the node and the function
                // behind it both rebuild at the destination.
                let knot = Coerced::tie(writer, f, f.ktype(), f.ktype(), f.ktype(), f.ktype());
                let barrier = Knotted::of(knot, 0);
                let source = context.lift::<KValueFamily>(Value::Knotted(barrier));
                let crossed = cross(context, dest, &source).unwrap();
                (context.keep(crossed), f.ktype())
            })
            .unwrap();
        graph.release(home, ReleaseAbsorption::IntoHolder).unwrap();
        graph
            .enter(dest, |context| {
                let carrier = context.redeem(dormant).unwrap();
                let Value::Knotted(barrier) = context.read(&carrier).value() else {
                    panic!("the kept barrier redeems as a knot member");
                };
                let node = barrier.coerced().expect("a barrier node");
                assert_eq!(node.ktype(), ktype);
                assert_eq!(node.declared(), ktype);
                let f = node.underlying();
                assert_eq!(f.member().knot().len(), 1);
                assert_eq!(
                    captured_value(fixture, f, "greeting").as_str(),
                    Some("hi"),
                    "the function behind the barrier rebuilt with its closure"
                );
            })
            .unwrap();
        graph.release(dest, ReleaseAbsorption::IntoHolder).unwrap();
        assert!(graph.is_empty());
    });
}
