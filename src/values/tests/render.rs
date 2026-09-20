//! The surface rendering `PRINT` writes.

use crate::parse::ExpressionPart;
use crate::symbols::BinderSymbol;
use crate::type_lattice::KType;
use crate::values::{Key, TypeValue};

use super::{Dict, Fixture, List, Record, Tagged, Value, pin, text, with_fixture};

fn rendered(fixture: &Fixture<'_, '_>, value: Value<'_, '_>) -> String {
    let mut out = String::new();
    value
        .render(&mut out, fixture.types, fixture.symbols, fixture.scratch())
        .unwrap();
    out
}

#[test]
fn scalars_render_bare() {
    with_fixture(|fixture| {
        fixture.in_cell(pin, |context| {
            assert_eq!(rendered(fixture, Value::Number(3.0)), "3");
            assert_eq!(rendered(fixture, Value::Number(2.5)), "2.5");
            assert_eq!(rendered(fixture, Value::Bool(true)), "true");
            assert_eq!(rendered(fixture, Value::Null), "null");
            assert_eq!(rendered(fixture, text(context.writer(), "hi")), "hi");
        })
    });
}

#[test]
fn containers_render_their_cells_in_reading_order() {
    with_fixture(|fixture| {
        let (types, scratch, symbols) = (fixture.types, fixture.scratch(), fixture.symbols);
        let zeta = BinderSymbol::declared("zeta", symbols).unwrap();
        let alpha = BinderSymbol::declared("alpha", symbols).unwrap();
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let cells = [Value::Number(1.0), text(writer, "a")];
            let list = Value::List(List::new(writer, cells.into_iter(), types, scratch));
            assert_eq!(rendered(fixture, list), "[1, a]");
            let entries = [
                (Key::str("b"), Value::Number(2.0)),
                (Key::number(1.0).unwrap(), list),
            ];
            let dict = Value::Dict(Dict::new(writer, &entries, types, scratch));
            assert_eq!(rendered(fixture, dict), "{1: [1, a], \"b\": 2}");
            let fields = [(zeta, Value::Null), (alpha, Value::Bool(false))];
            let record = Value::Record(Record::new(writer, &fields, types, scratch));
            assert_eq!(rendered(fixture, record), "{alpha = false, zeta = null}");
        })
    });
}

#[test]
fn types_tags_and_quotes_render_their_surface() {
    with_fixture(|fixture| {
        let ExpressionPart::QuotedExpression(node) = fixture.part("#(a b)") else {
            panic!("a quote parses to a quote part");
        };
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let number = Value::Type(TypeValue::new(writer, KType::NUMBER, fixture.types));
            assert_eq!(rendered(fixture, number), "Number");
            let tagged = Value::Tagged(Tagged::hold(writer, text(writer, "x"), KType::STR));
            assert_eq!(rendered(fixture, tagged), "Str(x)");
            assert_eq!(rendered(fixture, Value::Expression(node)), "a b");
        })
    });
}

#[test]
fn a_cycle_labels_its_target_and_a_shared_node_prints_inline() {
    use super::{Holding, Link, ring, tie};
    use crate::values::Circular;
    with_fixture(|fixture| {
        let (types, symbols, scratch) = (fixture.types, fixture.symbols, fixture.scratch());
        let ring_type = fixture.ring_type("Ring", "next");
        let next = BinderSymbol::declared("next", symbols).unwrap();
        let rendered = |value: Holding<'_, '_>| {
            let mut out = String::new();
            value.render(&mut out, types, symbols, scratch).unwrap();
            out
        };
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let one = ring(fixture, writer, ring_type, &[None])[0];
            assert_eq!(rendered(Holding::Knotted(one)), "@0 = Ring({next = @0})");
            let pair = ring(fixture, writer, ring_type, &[None, None]);
            assert_eq!(
                rendered(Holding::Knotted(pair[0])),
                "@0 = Ring({next = Ring({next = @0})})"
            );
            let held = crate::values::List::new(
                writer,
                [Holding::Knotted(one)].into_iter(),
                types,
                scratch,
            );
            assert_eq!(rendered(Holding::List(held)), "[@0 = Ring({next = @0})]");

            let record_type = types.record(scratch, &[(next, KType::NULL)]);
            let shared = tie(writer, 2, |index, edges| {
                if index == 0 {
                    Circular::List(crate::values::List::linked(
                        writer,
                        &[Link::Edge(edges[1]), Link::Edge(edges[1])],
                        types.list(record_type),
                    ))
                } else {
                    Circular::Record(crate::values::Record::linked(
                        writer,
                        &[(next, Link::Value(Holding::Null))],
                        record_type,
                        scratch,
                    ))
                }
            });
            assert_eq!(
                rendered(Holding::Knotted(shared[0])),
                "[{next = null}, {next = null}]"
            );
        })
    });
}
