//! The surface rendering `PRINT` writes.

use crate::parse::{BinderSymbol, ExpressionPart};
use crate::type_lattice::KType;
use crate::values::{Dict, Key, List, Record, Tagged, TypeValue, Value, text};

use super::{Fixture, pin, with_fixture};

fn rendered(fixture: &Fixture<'_, '_>, value: Value<'_, '_>) -> String {
    let mut out = String::new();
    value
        .render(&mut out, fixture.types, fixture.labels, fixture.scratch())
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
        let (types, scratch, labels) = (fixture.types, fixture.scratch(), fixture.labels);
        let zeta = BinderSymbol::declared("zeta", labels).unwrap();
        let alpha = BinderSymbol::declared("alpha", labels).unwrap();
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
