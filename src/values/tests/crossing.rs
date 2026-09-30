//! The crossing verb over both verdicts and both cell tiers, and the verdict itself.
//!
//! `a_copied_list_outlives_its_home` is on the Miri slate: it is the one path only `values` drives —
//! a deep copy laying down strings, a list and a dict whose string keys are written inside its key
//! run's `fill`, read after the region it was copied from is gone.

use std::ptr;

use crate::memory::{CellGraph, Prices, ReleaseAbsorption, Verdict};
use crate::symbols::BinderSymbol;
use crate::type_lattice::KType;
use crate::values::{COPY_RATIO, Key, ValueFamily, cross, cross_here, verdict};

use super::{Dict, List, Record, Step, Tagged, Value, copy, pin, text, with_fixture};

#[test]
fn a_copied_list_outlives_its_home() {
    with_fixture(|fixture| {
        let (types, scratch) = (fixture.types, fixture.scratch());
        let mut graph: CellGraph<'_, Step> = CellGraph::new(2, copy);
        let home = graph.create(None).unwrap();
        let dest = graph.create(None).unwrap();
        let dormant = graph
            .enter(home, |context| {
                let writer = context.writer();
                let words = [text(writer, "alpha"), text(writer, "beta")];
                let inner = List::new(writer, words.into_iter(), types, scratch);
                let entries = [(Key::str("key"), text(writer, "gamma"))];
                let dict = Dict::new(writer, &entries, types, scratch);
                let cells = [Value::List(inner), Value::Dict(dict)];
                let outer = List::new(writer, cells.into_iter(), types, scratch);
                let source = context.lift::<ValueFamily>(Value::List(outer));
                let crossed = cross(context, dest, &source, types).unwrap();
                let Value::List(copied) = context.read(&crossed).value() else {
                    panic!("a list crosses as a list");
                };
                assert!(!ptr::eq(copied, outer));
                context.keep(crossed)
            })
            .unwrap();
        graph.release(home, ReleaseAbsorption::IntoHolder).unwrap();
        graph
            .enter(dest, |context| {
                let carrier = context.redeem(dormant).unwrap();
                let Value::List(outer) = context.read(&carrier).value() else {
                    panic!("the kept list redeems as a list");
                };
                let inner = outer.get(0).and_then(Value::as_list).unwrap();
                assert_eq!(inner.get(1).and_then(Value::as_str), Some("beta"));
                let dict = outer.get(1).and_then(Value::as_dict).unwrap();
                assert_eq!(
                    dict.get(&Key::str("key")).and_then(Value::as_str),
                    Some("gamma")
                );
            })
            .unwrap();
        graph.release(dest, ReleaseAbsorption::IntoHolder).unwrap();
        assert!(graph.is_empty());
    });
}

#[test]
fn a_pinned_record_reads_after_its_home_seals() {
    with_fixture(|fixture| {
        let (types, scratch, symbols) = (fixture.types, fixture.scratch(), fixture.symbols);
        let name = BinderSymbol::declared("name", symbols).unwrap();
        let mut graph: CellGraph<'_, Step> = CellGraph::new(2, pin);
        let home = graph.create(None).unwrap();
        let holder = graph.create(None).unwrap();
        let dormant = graph
            .enter(home, |context| {
                let writer = context.writer();
                let fields = [(name, text(writer, "koan"))];
                let record = Record::new(writer, &fields, types, scratch);
                let source = context.lift::<ValueFamily>(Value::Record(record));
                let crossed = cross(context, holder, &source, types).unwrap();
                let Value::Record(pinned) = context.read(&crossed).value() else {
                    panic!("a record crosses as a record");
                };
                assert!(ptr::eq(pinned, record));
                context.keep(crossed)
            })
            .unwrap();
        graph.release(home, ReleaseAbsorption::Refused).unwrap();
        graph
            .enter(holder, |context| {
                let carrier = context.redeem(dormant).unwrap();
                let Value::Record(record) = context.read(&carrier).value() else {
                    panic!("the kept record redeems as a record");
                };
                assert_eq!(
                    record.field(name.symbol()).and_then(Value::as_str),
                    Some("koan")
                );
            })
            .unwrap();
        graph
            .release(holder, ReleaseAbsorption::IntoHolder)
            .unwrap();
        assert!(graph.is_empty());
    });
}

#[test]
fn a_kept_value_redeems_in_a_later_step() {
    with_fixture(|fixture| {
        let (types, scratch) = (fixture.types, fixture.scratch());
        let mut graph: CellGraph<'_, Step> = CellGraph::new(1, pin);
        let cell = graph.create(None).unwrap();
        let dormant = graph
            .enter(cell, |context| {
                let writer = context.writer();
                let entries = [
                    (Key::number(2.0).unwrap(), text(writer, "two")),
                    (Key::number(1.0).unwrap(), text(writer, "one")),
                ];
                let dict = Dict::new(writer, &entries, types, scratch);
                let carrier = context.lift::<ValueFamily>(Value::Dict(dict));
                context.keep(carrier)
            })
            .unwrap();
        graph
            .enter(cell, |context| {
                let carrier = context.redeem(dormant).unwrap();
                let Value::Dict(dict) = context.read(&carrier).value() else {
                    panic!("the kept dict redeems as a dict");
                };
                let texts: Vec<&str> = dict
                    .entries()
                    .filter_map(|(_, cell)| cell.as_str())
                    .collect();
                assert_eq!(texts, ["one", "two"]);
            })
            .unwrap();
        graph.release(cell, ReleaseAbsorption::IntoHolder).unwrap();
    });
}

#[test]
fn crossing_here_brings_a_value_back_into_the_step() {
    with_fixture(|fixture| {
        let (types, scratch) = (fixture.types, fixture.scratch());
        let mut graph: CellGraph<'_, Step> = CellGraph::new(2, copy);
        let cell = graph.create(None).unwrap();
        let other = graph.create(None).unwrap();
        graph
            .enter(cell, |context| {
                let writer = context.writer();
                let list = List::new(writer, [text(writer, "here")].into_iter(), types, scratch);
                let source = context.lift::<ValueFamily>(Value::List(list));
                let away = cross(context, other, &source, types).unwrap();
                let Value::List(back) = cross_here(context, &away, types) else {
                    panic!("a list crosses as a list");
                };
                // The copy is built through this step's writer, so it embeds in what the step builds.
                let wrapped = List::new(
                    context.writer(),
                    [Value::List(back)].into_iter(),
                    types,
                    scratch,
                );
                assert_eq!(wrapped.ktype(), types.list(back.ktype()));
                assert_eq!(back.get(0).and_then(Value::as_str), Some("here"));
            })
            .unwrap();
        graph.release(other, ReleaseAbsorption::IntoHolder).unwrap();
        graph.release(cell, ReleaseAbsorption::IntoHolder).unwrap();
        assert!(graph.is_empty());
    });
}

fn prices(pin_bytes: usize, copy_bytes: usize) -> Prices {
    Prices {
        pin_bytes,
        copy_bytes,
        occupied: 0,
        cap: 1,
        sealed_cells: 0,
        retained_bytes: 0,
        destination_bytes: 0,
    }
}

#[test]
fn verdict_pins_the_large() {
    assert_eq!(verdict(prices(4096, 1024)), Verdict::Pin);
    assert_eq!(verdict(prices(0, 0)), Verdict::Pin);
    assert_eq!(verdict(prices(usize::MAX, usize::MAX)), Verdict::Pin);
}

#[test]
fn verdict_copies_the_small() {
    assert_eq!(verdict(prices(4096, 4096 / COPY_RATIO - 1)), Verdict::Copy);
    assert_eq!(verdict(prices(usize::MAX, 1)), Verdict::Copy);
}

#[test]
fn a_circular_value_copies_as_the_same_graph_and_pins_as_the_same_node() {
    use super::{Holding, Link, NodeFamily, ring};
    use crate::values::{Circular, Knotted as _};
    with_fixture(|fixture| {
        let (types, scratch) = (fixture.types, fixture.scratch());
        for (verdict, copies) in [(copy as fn(Prices) -> Verdict, true), (pin, false)] {
            let mut graph: CellGraph<'_, Step> = CellGraph::new(2, verdict);
            let home = graph.create(None).unwrap();
            let dest = graph.create(None).unwrap();
            graph
                .enter(home, |context| {
                    let writer = context.writer();
                    let source = ring(fixture, writer, KType::STR, &[Some(text_in(writer)), None]);
                    let carrier =
                        context.lift::<ValueFamily<NodeFamily>>(Holding::Knotted(source[0]));
                    let crossed = cross(context, dest, &carrier, types).unwrap();
                    let Holding::Knotted(copied) = context.read(&crossed).value() else {
                        panic!("a knot member crosses as a knot member");
                    };
                    let equal = Holding::Knotted(copied).equals(
                        &Holding::Knotted(source[0]),
                        types,
                        scratch,
                    );
                    assert_eq!(equal, Ok(true));
                    if !copies {
                        assert!(copied == source[0], "a pin is the same node");
                        return;
                    }
                    assert!(copied != source[0], "a copy is a new knot");
                    let edge = |member: super::Node<'_>| {
                        let Some((_, Circular::Tagged(tagged))) =
                            Holding::Knotted(member).as_circular()
                        else {
                            panic!("a ring member is a tagged node");
                        };
                        match tagged.payload() {
                            Link::Edge(edge) => *edge,
                            Link::Value(_) => panic!("a ring payload is an edge"),
                        }
                    };
                    assert_eq!(edge(copied), edge(source[0]), "edges carry verbatim");
                    assert_eq!(copied.ktype(), source[0].ktype());
                })
                .unwrap();
            graph.release(dest, ReleaseAbsorption::IntoHolder).unwrap();
            graph.release(home, ReleaseAbsorption::IntoHolder).unwrap();
            assert!(graph.is_empty());
        }
    });
}

#[test]
fn two_members_of_one_knot_cross_as_members_of_one_copy() {
    use super::{Holding, NodeFamily, ring};
    with_fixture(|fixture| {
        let (types, scratch) = (fixture.types, fixture.scratch());
        let mut graph: CellGraph<'_, Step> = CellGraph::new(2, copy);
        let home = graph.create(None).unwrap();
        let dest = graph.create(None).unwrap();
        graph
            .enter(home, |context| {
                let writer = context.writer();
                let source = ring(fixture, writer, KType::STR, &[None, None]);
                let pair = [Holding::Knotted(source[0]), Holding::Knotted(source[1])];
                let list = crate::values::List::new(writer, pair.into_iter(), types, scratch);
                let carrier = context.lift::<ValueFamily<NodeFamily>>(Holding::List(list));
                let crossed = cross(context, dest, &carrier, types).unwrap();
                let copied = context.read(&crossed).value().as_list().expect("a list");
                let [Holding::Knotted(first), Holding::Knotted(second)] = copied.cells() else {
                    panic!("a list of two knot members");
                };
                assert!(first.0.knot() == second.0.knot(), "one copy of the knot");
                assert!(first.0.knot() != source[0].0.knot(), "a copy is a new knot");
                assert_eq!(first.0.index(), source[0].0.index());
                assert_eq!(second.0.index(), source[1].0.index());
            })
            .unwrap();
        graph.release(dest, ReleaseAbsorption::IntoHolder).unwrap();
        graph.release(home, ReleaseAbsorption::IntoHolder).unwrap();
        assert!(graph.is_empty());
    });
}

fn text_in<'cell>(writer: crate::memory::Writer<'cell>) -> super::Holding<'cell> {
    crate::values::text(writer, "held")
}

#[test]
fn a_copy_lays_down_only_what_a_retype_shows() {
    with_fixture(|fixture| {
        let (types, scratch, symbols) = (fixture.types, fixture.scratch(), fixture.symbols);
        let x = BinderSymbol::declared("x", symbols).unwrap();
        let y = BinderSymbol::declared("y", symbols).unwrap();
        let narrow = types.record(scratch, &[(x, KType::NUMBER)]);
        for (verdict, copies) in [(copy as fn(Prices) -> Verdict, true), (pin, false)] {
            let mut graph: CellGraph<'_, Step> = CellGraph::new(2, verdict);
            let home = graph.create(None).unwrap();
            let dest = graph.create(None).unwrap();
            graph
                .enter(home, |context| {
                    let writer = context.writer();
                    let fields = [(x, Value::Number(1.0)), (y, text(writer, "a"))];
                    let record = Record::new(writer, &fields, types, scratch);
                    let retyped = Value::Record(record).retyped(writer, narrow, types, scratch);
                    let source = context.lift::<ValueFamily>(retyped);
                    let crossed = cross(context, dest, &source, types).unwrap();
                    let Value::Record(copied) = context.read(&crossed).value() else {
                        panic!("a record crosses as a record");
                    };
                    let mut rendered = String::new();
                    Value::Record(copied)
                        .render(&mut rendered, types, symbols, scratch)
                        .unwrap();
                    assert_eq!(rendered, "{x = 1}");
                    if !copies {
                        assert!(ptr::eq(copied, retyped.as_record().unwrap()));
                        return;
                    }
                    assert_eq!(copied.names().len(), 1);
                    assert_eq!(copied.ktype(), narrow);
                    let fresh = Record::new(writer, &[(x, Value::Number(1.0))], types, scratch);
                    assert_eq!(copied.weight(), fresh.weight());
                })
                .unwrap();
            graph.release(dest, ReleaseAbsorption::IntoHolder).unwrap();
            graph.release(home, ReleaseAbsorption::IntoHolder).unwrap();
            assert!(graph.is_empty());
        }
    });
}

#[test]
fn a_copy_lays_down_each_part_at_the_type_its_holder_names() {
    with_fixture(|fixture| {
        let (types, scratch, symbols) = (fixture.types, fixture.scratch(), fixture.symbols);
        let field = |name| BinderSymbol::declared(name, symbols).unwrap();
        let (x, y, z) = (field("x"), field("y"), field("z"));
        let narrow = types.record(scratch, &[(x, KType::NUMBER)]);
        let representation = types.record(scratch, &[(x, KType::NUMBER), (y, KType::NUMBER)]);
        let point = fixture.newtype("Point", representation);
        let mut graph: CellGraph<'_, Step> = CellGraph::new(2, copy);
        let home = graph.create(None).unwrap();
        let dest = graph.create(None).unwrap();
        graph
            .enter(home, |context| {
                let writer = context.writer();
                let fields = [(x, Value::Number(1.0)), (y, Value::Number(2.0))];
                let element = Value::Record(Record::new(writer, &fields, types, scratch));
                let list = List::new(writer, [element].into_iter(), types, scratch);
                let retyped = Value::List(list).retyped(writer, types.list(narrow), types, scratch);
                let source = context.lift::<ValueFamily>(retyped);
                let crossed = cross(context, dest, &source, types).unwrap();
                let copied = context.read(&crossed).value().as_list().expect("a list");
                let element = copied.get(0).and_then(Value::as_record).expect("a record");
                assert_eq!(element.cells().len(), 1);
                assert_eq!(element.ktype(), narrow);

                let fields = [
                    (x, Value::Number(1.0)),
                    (y, Value::Number(2.0)),
                    (z, Value::Number(3.0)),
                ];
                let payload = Value::Record(Record::new(writer, &fields, types, scratch));
                let tagged = Value::Tagged(Tagged::hold(writer, payload, point));
                let source = context.lift::<ValueFamily>(tagged);
                let crossed = cross(context, dest, &source, types).unwrap();
                let Value::Tagged(copied) = context.read(&crossed).value() else {
                    panic!("a tagged value crosses as a tagged value");
                };
                assert_eq!(copied.ktype(), point);
                let payload = copied.payload().as_record().expect("a record payload");
                assert_eq!(payload.cells().len(), 2);
                assert_eq!(payload.ktype(), representation);
            })
            .unwrap();
        graph.release(dest, ReleaseAbsorption::IntoHolder).unwrap();
        graph.release(home, ReleaseAbsorption::IntoHolder).unwrap();
        assert!(graph.is_empty());
    });
}

/// A data node seen at a type other than its memo copies as a plain value of its kind at that
/// type, as a retype lays it down, not as a member of a copy of its knot.
#[test]
fn a_data_node_seen_at_another_type_copies_as_a_plain_value() {
    use super::{Holding, Link, NodeFamily, tie};
    use crate::values::Circular;
    with_fixture(|fixture| {
        let (types, scratch, symbols) = (fixture.types, fixture.scratch(), fixture.symbols);
        let field = |name| BinderSymbol::declared(name, symbols).unwrap();
        let (next, value, held) = (field("next"), field("value"), field("held"));
        let memo = types.record(scratch, &[(next, KType::ANY), (value, KType::NUMBER)]);
        let narrow = types.record(scratch, &[(value, KType::NUMBER)]);
        let mut graph: CellGraph<'_, Step> = CellGraph::new(2, copy);
        let home = graph.create(None).unwrap();
        let dest = graph.create(None).unwrap();
        graph
            .enter(home, |context| {
                let writer = context.writer();
                let node = tie(writer, 1, |_, edges| {
                    let fields = [
                        (next, Link::Edge(edges[0])),
                        (value, Link::Value(Holding::Number(1.0))),
                    ];
                    Circular::Record(crate::values::Record::linked(
                        writer, &fields, memo, scratch,
                    ))
                })[0];
                let fields = [(held, Holding::Knotted(node))];
                let holder = crate::values::Record::new(writer, &fields, types, scratch);
                let shown = types.record(scratch, &[(held, narrow)]);
                let retyped = Holding::Record(holder).retyped(writer, shown, types, scratch);
                let source = context.lift::<ValueFamily<NodeFamily>>(retyped);
                let crossed = cross(context, dest, &source, types).unwrap();
                let copied = context
                    .read(&crossed)
                    .value()
                    .as_record()
                    .expect("a record");
                let Some(Holding::Record(laid)) = copied.field(held.symbol()) else {
                    panic!("the node copies as a plain record");
                };
                assert_eq!(laid.ktype(), narrow);
                assert_eq!(laid.names(), &[value.symbol()]);
            })
            .unwrap();
        graph.release(dest, ReleaseAbsorption::IntoHolder).unwrap();
        graph.release(home, ReleaseAbsorption::IntoHolder).unwrap();
        assert!(graph.is_empty());
    });
}
