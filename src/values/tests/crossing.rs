//! The crossing verb over both verdicts and both cell tiers, and the verdict itself.
//!
//! `a_copied_list_outlives_its_home` is on the Miri slate: it is the one path only `values` drives —
//! a deep copy nesting `fill` inside `fill` with string writes between, a program node embedded,
//! read after the region it was copied from is gone.

use std::ptr;

use crate::memory::{CellGraph, Prices, ReleaseAbsorption, Verdict};
use crate::parse::{BinderSymbol, ExpressionPart, ProgramNode};
use crate::values::{
    COPY_RATIO, Dict, Key, List, Record, Value, ValueFamily, cross, cross_here, text, verdict,
};

use super::{Fixture, Step, copy, pin, with_fixture};

fn quote<'graph>(fixture: &Fixture<'_, 'graph>, source: &str) -> ProgramNode<'graph> {
    match fixture.part(source) {
        ExpressionPart::QuotedExpression(node) => node,
        _ => panic!("`{source}` parses to a quote"),
    }
}

#[test]
fn a_copied_list_outlives_its_home() {
    with_fixture(|fixture| {
        let (types, scratch) = (fixture.types, fixture.scratch());
        let node = quote(fixture, "#(a b)");
        let mut graph: CellGraph<'_, Step> = CellGraph::new(2, copy);
        let home = graph.create(None, None).unwrap();
        let dest = graph.create(None, None).unwrap();
        let dormant = graph
            .enter(home, |context| {
                let writer = context.writer();
                let words = [text(writer, "alpha"), text(writer, "beta")];
                let inner = List::new(writer, words.into_iter(), types, scratch);
                let entries = [(Key::str("key"), text(writer, "gamma"))];
                let dict = Dict::new(writer, &entries, types, scratch);
                let cells = [
                    Value::List(inner),
                    Value::Dict(dict),
                    Value::Expression(node),
                ];
                let outer = List::new(writer, cells.into_iter(), types, scratch);
                let source = context.lift::<ValueFamily>(Value::List(outer));
                let crossed = cross(context, dest, &source).unwrap();
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
                let copied_node = outer.get(2).and_then(Value::as_expression).unwrap();
                assert!(ptr::eq(copied_node.reference(), node.reference()));
            })
            .unwrap();
        graph.release(dest, ReleaseAbsorption::IntoHolder).unwrap();
        assert!(graph.is_empty());
    });
}

#[test]
fn a_pinned_record_reads_after_its_home_seals() {
    with_fixture(|fixture| {
        let (types, scratch, labels) = (fixture.types, fixture.scratch(), fixture.labels);
        let name = BinderSymbol::declared("name", labels).unwrap();
        let mut graph: CellGraph<'_, Step> = CellGraph::new(2, pin);
        let home = graph.create(None, None).unwrap();
        let holder = graph.create(None, None).unwrap();
        let dormant = graph
            .enter(home, |context| {
                let writer = context.writer();
                let fields = [(name, text(writer, "koan"))];
                let record = Record::new(writer, &fields, types, scratch);
                let source = context.lift::<ValueFamily>(Value::Record(record));
                let crossed = cross(context, holder, &source).unwrap();
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
        let cell = graph.create(None, None).unwrap();
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
fn a_quote_crosses_a_forced_tree_copy() {
    with_fixture(|fixture| {
        let (types, scratch) = (fixture.types, fixture.scratch());
        let node = quote(fixture, "#(x)");
        let mut graph: CellGraph<'_, Step> = CellGraph::new(1, pin);
        let root = graph.create(None, None).unwrap();
        let left = graph.create_tree(root, None).unwrap();
        let right = graph.create_tree(root, None).unwrap();
        graph
            .enter(left, |context| {
                let writer = context.writer();
                let cells = [Value::Expression(node), text(writer, "left")];
                let list = List::new(writer, cells.into_iter(), types, scratch);
                let source = context.lift::<ValueFamily>(Value::List(list));
                let crossed = cross(context, right, &source).unwrap();
                let Value::List(copied) = context.read(&crossed).value() else {
                    panic!("a list crosses as a list");
                };
                assert!(!ptr::eq(copied.cells(), list.cells()));
                let copied_node = copied.get(0).and_then(Value::as_expression).unwrap();
                assert!(ptr::eq(copied_node.reference(), node.reference()));
                assert_eq!(copied.get(1).and_then(Value::as_str), Some("left"));
            })
            .unwrap();
        graph.release_tree(right).unwrap();
        graph.release_tree(left).unwrap();
        graph.release(root, ReleaseAbsorption::IntoHolder).unwrap();
        assert!(graph.is_empty());
    });
}

#[test]
fn crossing_here_brings_a_value_back_into_the_step() {
    with_fixture(|fixture| {
        let (types, scratch) = (fixture.types, fixture.scratch());
        let mut graph: CellGraph<'_, Step> = CellGraph::new(2, copy);
        let cell = graph.create(None, None).unwrap();
        let other = graph.create(None, None).unwrap();
        graph
            .enter(cell, |context| {
                let writer = context.writer();
                let list = List::new(writer, [text(writer, "here")].into_iter(), types, scratch);
                let source = context.lift::<ValueFamily>(Value::List(list));
                let away = cross(context, other, &source).unwrap();
                let Value::List(back) = cross_here(context, &away) else {
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
