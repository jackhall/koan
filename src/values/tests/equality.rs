//! Structural equality: IEEE numbers, nominal identity first, containers gated on related types,
//! quotes compared as syntax.

use crate::parse::{BinderSymbol, ExpressionPart};
use crate::type_lattice::KType;
use crate::values::{Dict, Key, List, Record, Tagged, TypeValue, Value, text};

use super::{pin, with_fixture};

#[test]
fn scalars_compare_by_ieee_and_types_by_handle() {
    with_fixture(|fixture| {
        let (types, scratch) = (fixture.types, fixture.scratch());
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let equal =
                |left: &Value<'_, '_>, right: &Value<'_, '_>| left.equals(right, types, scratch);
            assert!(!equal(&Value::Number(f64::NAN), &Value::Number(f64::NAN)));
            assert!(equal(&Value::Number(-0.0), &Value::Number(0.0)));
            assert!(equal(&text(writer, "a"), &text(writer, "a")));
            assert!(!equal(&text(writer, "a"), &Value::Number(1.0)));
            assert!(equal(&Value::Null, &Value::Null));
            let number = Value::Type(TypeValue::new(writer, KType::NUMBER, types));
            let again = Value::Type(TypeValue::new(writer, KType::NUMBER, types));
            let string = Value::Type(TypeValue::new(writer, KType::STR, types));
            assert!(equal(&number, &again));
            assert!(!equal(&number, &string));
        })
    });
}

#[test]
fn containers_compare_contents_only_under_related_types() {
    with_fixture(|fixture| {
        let (types, scratch, labels) = (fixture.types, fixture.scratch(), fixture.labels);
        let x = BinderSymbol::declared("x", labels).unwrap();
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let equal =
                |left: Value<'_, '_>, right: Value<'_, '_>| left.equals(&right, types, scratch);
            let list =
                |items: &[_]| Value::List(List::new(writer, items.iter().copied(), types, scratch));
            let one_two = list(&[Value::Number(1.0), Value::Number(2.0)]);
            assert!(equal(
                one_two,
                list(&[Value::Number(1.0), Value::Number(2.0)])
            ));
            assert!(!equal(
                one_two,
                list(&[Value::Number(2.0), Value::Number(1.0)])
            ));
            assert!(!equal(one_two, list(&[Value::Number(1.0)])));
            // A retype to a related type still compares; an empty list of an unrelated element type
            // does not.
            assert!(equal(
                one_two,
                one_two.retyped(writer, KType::LIST_OF_ANY, types)
            ));
            let empty = Value::List(List::new(writer, [].into_iter(), types, scratch));
            let empty_strings = empty.retyped(writer, types.list(KType::STR), types);
            let empty_numbers = empty.retyped(writer, types.list(KType::NUMBER), types);
            assert!(!equal(empty_strings, empty_numbers));

            let dict = |entries: &[_]| Value::Dict(Dict::new(writer, entries, types, scratch));
            assert!(equal(
                dict(&[
                    (Key::str("a"), Value::Number(1.0)),
                    (Key::str("b"), Value::Null)
                ]),
                dict(&[
                    (Key::str("b"), Value::Null),
                    (Key::str("a"), Value::Number(1.0))
                ]),
            ));
            assert!(!equal(
                dict(&[(Key::str("a"), Value::Number(1.0))]),
                dict(&[(Key::str("a"), Value::Number(2.0))]),
            ));

            let record = |value| Value::Record(Record::new(writer, &[(x, value)], types, scratch));
            assert!(equal(record(Value::Bool(true)), record(Value::Bool(true))));
            assert!(!equal(
                record(Value::Bool(true)),
                record(Value::Bool(false))
            ));
        })
    });
}

#[test]
fn a_tagged_value_never_equals_its_payload() {
    with_fixture(|fixture| {
        let (types, scratch) = (fixture.types, fixture.scratch());
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let tagged =
                |identity| Value::Tagged(Tagged::hold(writer, Value::Number(1.0), identity));
            assert!(tagged(KType::STR).equals(&tagged(KType::STR), types, scratch));
            assert!(!tagged(KType::STR).equals(&tagged(KType::BOOL), types, scratch));
            assert!(!tagged(KType::STR).equals(&Value::Number(1.0), types, scratch));
        })
    });
}

#[test]
fn quotes_compare_as_syntax() {
    with_fixture(|fixture| {
        let (types, scratch) = (fixture.types, fixture.scratch());
        let quote = |source| match fixture.part(source) {
            ExpressionPart::QuotedExpression(node) => Value::Expression(node),
            _ => panic!("`{source}` parses to a quote"),
        };
        assert!(quote("#(a [1 2] {x = \"s\"})").equals(
            &quote("#(a [1 2] {x = \"s\"})"),
            types,
            scratch
        ));
        assert!(!quote("#(a [1 2])").equals(&quote("#(a [2 1])"), types, scratch));
        assert!(!quote("#(a)").equals(&quote("#(a b)"), types, scratch));
    });
}
