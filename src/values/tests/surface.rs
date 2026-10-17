//! The door: a value seen at a type, the surface it opens at that type, and a retype as the two
//! composed.

use std::ptr;

use crate::symbols::BinderSymbol;
use crate::type_lattice::KType;
use crate::values::Seen;

use super::{List, Record, Tagged, Value, pin, text, with_fixture};

#[test]
fn a_retype_is_a_seen_type_restamped_over_the_same_runs() {
    with_fixture(|fixture| {
        fixture.in_cell(pin, |context| {
            let types = &fixture.tiers.of(context);
            let writer = context.writer();
            let (types, scratch, symbols) = (types, fixture.scratch(), fixture.symbols);
            let x = BinderSymbol::declared("x", symbols).unwrap();
            let y = BinderSymbol::declared("y", symbols).unwrap();
            let fields = [(x, Value::Number(1.0)), (y, text(writer, "a"))];
            let record = Record::new(writer, &fields, types, scratch);
            let narrow = types.record(scratch, &[(x, KType::NUMBER)]);
            let seen = Seen::of(Value::Record(record)).seen_at(narrow, types, scratch);
            assert_eq!(seen.ktype(), narrow.into());
            let Value::Record(restamped) = seen.restamped(writer, types) else {
                panic!("a record restamps as a record");
            };
            let Value::Record(retyped) =
                Value::Record(record).retyped(writer, narrow, types, scratch)
            else {
                panic!("a record retypes as a record");
            };
            for other in [restamped, retyped] {
                assert_eq!(other.ktype(), narrow);
                assert!(ptr::eq(other.names(), record.names()));
                assert!(ptr::eq(other.cells(), record.cells()));
            }
        })
    });
}

#[test]
fn a_record_shows_only_the_fields_its_seen_type_names() {
    with_fixture(|fixture| {
        fixture.in_cell(pin, |context| {
            let types = &fixture.tiers.of(context);
            let writer = context.writer();
            let (types, scratch, symbols) = (types, fixture.scratch(), fixture.symbols);
            let x = BinderSymbol::declared("x", symbols).unwrap();
            let y = BinderSymbol::declared("y", symbols).unwrap();
            let fields = [(x, Value::Number(1.0)), (y, text(writer, "a"))];
            let record = Record::new(writer, &fields, types, scratch);
            let narrow = types.record(scratch, &[(x, KType::NUMBER)]);
            let retyped = Value::Record(record).retyped(writer, narrow, types, scratch);
            let surface = retyped.surface(types, scratch).expect("a record opens");
            assert_eq!(surface.ktype(), narrow);
            assert_eq!(surface.len(), 1);
            assert_eq!(surface.name(0), x.symbol());
            assert!(retyped.field(y.symbol(), types, scratch).is_none());
            let read = retyped
                .field(x.symbol(), types, scratch)
                .expect("the type names `x`");
            assert_eq!(read.ktype(), KType::NUMBER.into());
            assert!(matches!(read.value(), Value::Number(1.0)));
        })
    });
}

#[test]
fn an_element_is_seen_at_the_type_its_list_names() {
    with_fixture(|fixture| {
        fixture.in_cell(pin, |context| {
            let types = &fixture.tiers.of(context);
            let writer = context.writer();
            let (types, scratch, symbols) = (types, fixture.scratch(), fixture.symbols);
            let x = BinderSymbol::declared("x", symbols).unwrap();
            let y = BinderSymbol::declared("y", symbols).unwrap();
            let fields = [(x, Value::Number(1.0)), (y, Value::Number(2.0))];
            let record = Value::Record(Record::new(writer, &fields, types, scratch));
            let list = List::new(writer, [record].into_iter(), types, scratch);
            let narrow = types.record(scratch, &[(x, KType::NUMBER)]);
            let retyped = Value::List(list).retyped(writer, types.list(narrow), types, scratch);
            let surface = retyped.surface(types, scratch).expect("a list opens");
            let element = surface.child(0, types, scratch);
            assert_eq!(element.ktype(), narrow.into());
            assert_eq!(
                element.value().ktype(),
                record.ktype(),
                "a read restamps nothing"
            );
            let opened = element.surface(types, scratch).expect("a record opens");
            assert_eq!(opened.len(), 1);
        })
    });
}

#[test]
fn a_payload_is_seen_at_its_identitys_representation() {
    with_fixture(|fixture| {
        let (types, scratch, symbols) = (fixture.types, fixture.scratch(), fixture.symbols);
        let field = |name| BinderSymbol::declared(name, symbols).unwrap();
        let (x, y, z) = (field("x"), field("y"), field("z"));
        let representation = types.record(scratch, &[(x, KType::NUMBER), (y, KType::NUMBER)]);
        let point = fixture.newtype("Point", representation);
        fixture.in_cell(pin, |context| {
            let types = &fixture.tiers.of(context);
            let writer = context.writer();
            let fields = [
                (x, Value::Number(1.0)),
                (y, Value::Number(2.0)),
                (z, Value::Number(3.0)),
            ];
            let payload = Value::Record(Record::new(writer, &fields, types, scratch));
            let tagged = Value::Tagged(Tagged::hold(writer, payload, point, types));
            let surface = tagged
                .surface(types, scratch)
                .expect("a tagged value opens");
            assert_eq!(surface.len(), 1);
            let payload = surface.child(0, types, scratch);
            assert_eq!(payload.ktype(), representation.into());
            let opened = payload.surface(types, scratch).expect("a record opens");
            assert_eq!(opened.len(), 2);
        })
    });
}

/// A one-node knot: a record node under `{next :Any, value :Number}` whose `next` names itself.
pub(super) fn looped<'cell>(
    fixture: &super::Fixture<'_, '_>,
    writer: crate::memory::Writer<'cell, '_>,
    types: &crate::values::Chain<'_, '_, 'cell, '_, '_>,
    value: f64,
) -> super::Node<'cell> {
    let (scratch, symbols) = (fixture.scratch(), fixture.symbols);
    let next = BinderSymbol::declared("next", symbols).unwrap();
    let held = BinderSymbol::declared("value", symbols).unwrap();
    let memo = types.record(scratch, &[(next, KType::ANY), (held, KType::NUMBER)]);
    super::tie(writer, 1, |_, edges| {
        let fields = [
            (next, super::Link::Edge(edges[0])),
            (held, super::Link::Value(super::Holding::Number(value))),
        ];
        crate::values::Circular::Record(crate::values::Record::linked(
            writer, &fields, memo, scratch,
        ))
    })[0]
}

#[test]
fn a_data_node_restamps_under_one_header_over_its_bare_member() {
    use super::Holding;
    with_fixture(|fixture| {
        fixture.in_cell(pin, |context| {
            let types = &fixture.tiers.of(context);
            let writer = context.writer();
            let (scratch, symbols) = (fixture.scratch(), fixture.symbols);
            let node = looped(fixture, writer, types, 1.0);
            let next = BinderSymbol::declared("next", symbols).unwrap();
            let narrow = types.record(scratch, &[(next, KType::ANY)]);
            let Holding::Retyped(header) =
                Holding::Knotted(node).retyped(writer, narrow, types, scratch)
            else {
                panic!("a data node restamps under a header");
            };
            assert!(header.member() == node);
            assert_eq!(header.ktype(), narrow.into());
            let any_record = types.record(scratch, &[]);
            let Holding::Retyped(again) =
                Holding::Retyped(header).retyped(writer, any_record, types, scratch)
            else {
                panic!("a header restamps under a header");
            };
            assert!(again.member() == node, "one header over the bare member");
            assert_eq!(again.ktype(), any_record.into());
            let own = crate::values::Knotted::ktype(&node).as_type().unwrap();
            let either = types.union_of(scratch, &[own, KType::NULL]);
            assert!(
                matches!(
                    Holding::Knotted(node).retyped(writer, either, types, scratch),
                    Holding::Knotted(member) if member == node
                ),
                "a retype to the node's own type writes no header"
            );
        })
    });
}
