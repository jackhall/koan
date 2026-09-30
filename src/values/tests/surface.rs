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
            let writer = context.writer();
            let (types, scratch, symbols) = (fixture.types, fixture.scratch(), fixture.symbols);
            let x = BinderSymbol::declared("x", symbols).unwrap();
            let y = BinderSymbol::declared("y", symbols).unwrap();
            let fields = [(x, Value::Number(1.0)), (y, text(writer, "a"))];
            let record = Record::new(writer, &fields, types, scratch);
            let narrow = types.record(scratch, &[(x, KType::NUMBER)]);
            let seen = Seen::of(Value::Record(record)).seen_at(narrow, types, scratch);
            assert_eq!(seen.ktype(), narrow);
            let Value::Record(restamped) = seen.restamped(writer) else {
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
            let writer = context.writer();
            let (types, scratch, symbols) = (fixture.types, fixture.scratch(), fixture.symbols);
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
            assert_eq!(read.ktype(), KType::NUMBER);
            assert!(matches!(read.value(), Value::Number(1.0)));
        })
    });
}

#[test]
fn an_element_is_seen_at_the_type_its_list_names() {
    with_fixture(|fixture| {
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let (types, scratch, symbols) = (fixture.types, fixture.scratch(), fixture.symbols);
            let x = BinderSymbol::declared("x", symbols).unwrap();
            let y = BinderSymbol::declared("y", symbols).unwrap();
            let fields = [(x, Value::Number(1.0)), (y, Value::Number(2.0))];
            let record = Value::Record(Record::new(writer, &fields, types, scratch));
            let list = List::new(writer, [record].into_iter(), types, scratch);
            let narrow = types.record(scratch, &[(x, KType::NUMBER)]);
            let retyped = Value::List(list).retyped(writer, types.list(narrow), types, scratch);
            let surface = retyped.surface(types, scratch).expect("a list opens");
            let element = surface.child(0, types, scratch);
            assert_eq!(element.ktype(), narrow);
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
            let writer = context.writer();
            let fields = [
                (x, Value::Number(1.0)),
                (y, Value::Number(2.0)),
                (z, Value::Number(3.0)),
            ];
            let payload = Value::Record(Record::new(writer, &fields, types, scratch));
            let tagged = Value::Tagged(Tagged::hold(writer, payload, point));
            let surface = tagged
                .surface(types, scratch)
                .expect("a tagged value opens");
            assert_eq!(surface.len(), 1);
            let payload = surface.child(0, types, scratch);
            assert_eq!(payload.ktype(), representation);
            let opened = payload.surface(types, scratch).expect("a record opens");
            assert_eq!(opened.len(), 2);
        })
    });
}
