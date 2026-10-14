//! The surface rendering `PRINT` writes.

use crate::symbols::BinderSymbol;
use crate::type_lattice::KType;
use crate::values::{Chain, Key, TypeValue};

use super::{Dict, Fixture, List, Record, Stand, Tagged, Value, pin, quote, text, with_fixture};

fn rendered(
    fixture: &Fixture<'_, '_>,
    types: &Chain<'_, '_, '_, '_, '_>,
    value: Value<'_>,
) -> String {
    let mut out = String::new();
    value
        .render(&mut out, types, fixture.symbols, fixture.scratch())
        .unwrap();
    out
}

#[test]
fn scalars_render_bare() {
    with_fixture(|fixture| {
        fixture.in_cell(pin, |context| {
            let types = &fixture.tiers.of(context);
            assert_eq!(rendered(fixture, types, Value::Number(3.0)), "3");
            assert_eq!(rendered(fixture, types, Value::Number(2.5)), "2.5");
            assert_eq!(rendered(fixture, types, Value::Bool(true)), "true");
            assert_eq!(rendered(fixture, types, Value::Null), "null");
            assert_eq!(rendered(fixture, types, text(context.writer(), "hi")), "hi");
        })
    });
}

#[test]
fn containers_render_their_cells_in_reading_order() {
    with_fixture(|fixture| {
        let (scratch, symbols) = (fixture.scratch(), fixture.symbols);
        let zeta = BinderSymbol::declared("zeta", symbols).unwrap();
        let alpha = BinderSymbol::declared("alpha", symbols).unwrap();
        fixture.in_cell(pin, |context| {
            let types = &fixture.tiers.of(context);
            let writer = context.writer();
            let cells = [Value::Number(1.0), text(writer, "a")];
            let list = Value::List(List::new(writer, cells.into_iter(), types, scratch));
            assert_eq!(rendered(fixture, types, list), "[1, a]");
            let entries = [
                (Key::str("b"), Value::Number(2.0)),
                (Key::number(1.0).unwrap(), list),
            ];
            let dict = Value::Dict(Dict::new(writer, &entries, types, scratch));
            assert_eq!(rendered(fixture, types, dict), "{1: [1, a], \"b\": 2}");
            let fields = [(zeta, Value::Null), (alpha, Value::Bool(false))];
            let record = Value::Record(Record::new(writer, &fields, types, scratch));
            assert_eq!(
                rendered(fixture, types, record),
                "{alpha = false, zeta = null}"
            );
        })
    });
}

#[test]
fn types_tags_and_quotes_render_their_surface() {
    with_fixture(|fixture| {
        fixture.in_cell(pin, |context| {
            let types = &fixture.tiers.of(context);
            let writer = context.writer();
            let number = Value::Type(TypeValue::new(writer, KType::NUMBER, types));
            assert_eq!(rendered(fixture, types, number), "Number");
            let tagged = Value::Tagged(Tagged::hold(writer, text(writer, "x"), KType::STR));
            assert_eq!(rendered(fixture, types, tagged), "Str(x)");
            let code = crate::values::Value::Knotted(Stand::Code(quote(fixture, "#($a \\b)")));
            let mut out = String::new();
            code.render(&mut out, types, fixture.symbols, fixture.scratch())
                .unwrap();
            assert_eq!(out, "$a \\b", "code prints each mark as written");
        })
    });
}

#[test]
fn a_cycle_labels_its_target_and_a_shared_node_prints_inline() {
    use super::{Holding, Link, ring, tie};
    use crate::values::Circular;
    with_fixture(|fixture| {
        let (symbols, scratch) = (fixture.symbols, fixture.scratch());
        let ring_type = fixture.ring_type("Ring", "next");
        let next = BinderSymbol::declared("next", symbols).unwrap();
        fixture.in_cell(pin, |context| {
            let types = &fixture.tiers.of(context);
            let rendered = |value: Holding<'_>| {
                let mut out = String::new();
                value.render(&mut out, types, symbols, scratch).unwrap();
                out
            };
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

#[test]
fn a_header_over_a_cyclic_node_renders_its_cycle_labelled_at_the_top() {
    use super::{Holding, Link, ring};
    use crate::values::{Circular, Knotted};
    with_fixture(|fixture| {
        let (symbols, scratch) = (fixture.symbols, fixture.scratch());
        let ring_type = fixture.ring_type("Ring", "next");
        let next = BinderSymbol::declared("next", symbols).unwrap();
        fixture.in_cell(pin, |context| {
            let types = &fixture.tiers.of(context);
            let writer = context.writer();
            let member = ring(fixture, writer, ring_type, &[Some(Holding::Number(1.0))])[0];
            let Some((_, Circular::Tagged(tagged))) = Holding::Knotted(member).as_circular() else {
                panic!("a ring member is a tagged node");
            };
            let Link::Edge(payload) = *tagged.payload() else {
                panic!("over an edge to its record node");
            };
            let record = member.sibling(payload);
            let representation = types.record(scratch, &[(next, ring_type)]);
            let header = Holding::Knotted(record).retyped(writer, representation, types, scratch);
            assert!(matches!(header, Holding::Retyped(_)));
            let mut out = String::new();
            header.render(&mut out, types, symbols, scratch).unwrap();
            assert_eq!(out, "@0 = {next = Ring(@0)}");
        })
    });
}
