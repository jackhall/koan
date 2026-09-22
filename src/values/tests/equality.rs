//! Structural equality: IEEE numbers, nominal identity first, containers gated on related types,
//! quotes compared as syntax.

use crate::parse::ExpressionPart;
use crate::symbols::BinderSymbol;
use crate::type_lattice::KType;
use crate::values::{Key, TypeValue};

use super::{Dict, List, Record, Tagged, Value, pin, text, with_fixture};

#[test]
fn scalars_compare_by_ieee_and_types_by_handle() {
    with_fixture(|fixture| {
        let (types, scratch) = (fixture.types, fixture.scratch());
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let equal = |left: &Value<'_, '_>, right: &Value<'_, '_>| {
                left.equals(right, types, scratch).expect("no callable")
            };
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
        let (types, scratch, symbols) = (fixture.types, fixture.scratch(), fixture.symbols);
        let x = BinderSymbol::declared("x", symbols).unwrap();
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let equal = |left: Value<'_, '_>, right: Value<'_, '_>| {
                left.equals(&right, types, scratch).expect("no callable")
            };
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
            assert!(
                tagged(KType::STR)
                    .equals(&tagged(KType::STR), types, scratch)
                    .expect("no callable")
            );
            assert!(
                !tagged(KType::STR)
                    .equals(&tagged(KType::BOOL), types, scratch)
                    .expect("no callable")
            );
            assert!(
                !tagged(KType::STR)
                    .equals(&Value::Number(1.0), types, scratch)
                    .expect("no callable")
            );
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
        assert!(
            quote("#(a [1 2] {x = \"s\"})")
                .equals(&quote("#(a [1 2] {x = \"s\"})"), types, scratch)
                .expect("no callable")
        );
        assert!(
            !quote("#(a [1 2])")
                .equals(&quote("#(a [2 1])"), types, scratch)
                .expect("no callable")
        );
        assert!(
            !quote("#(a)")
                .equals(&quote("#(a b)"), types, scratch)
                .expect("no callable")
        );
    });
}

/// A stand-in function: `values` compares none, so all it needs is a type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Opaque;

impl crate::values::Knotted for Opaque {
    fn ktype(&self) -> KType {
        KType::ANY
    }

    fn weight(&self) -> crate::values::Weight {
        crate::values::Weight::ZERO
    }

    fn sibling(&self, _: crate::memory::Edge) -> Self {
        *self
    }

    fn resolve<'a>(&self) -> crate::values::Resolved<'a, Self> {
        crate::values::Resolved::Function
    }
}

#[test]
fn a_comparison_reaching_a_callable_is_incomparable() {
    use crate::values::{Incomparable, Value as Holding};
    with_fixture(|fixture| {
        let (types, scratch) = (fixture.types, fixture.scratch());
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let callable = Holding::Knotted(Opaque);
            let list = |items: &[_]| {
                Holding::List(crate::values::List::new(
                    writer,
                    items.iter().copied(),
                    types,
                    scratch,
                ))
            };
            assert_eq!(
                callable.equals(&callable, types, scratch),
                Err(Incomparable)
            );
            assert_eq!(
                Value::Number(1.0).equals(&callable, types, scratch),
                Err(Incomparable)
            );
            assert_eq!(
                list(&[Holding::Number(1.0), callable]).equals(
                    &list(&[Holding::Number(2.0), callable]),
                    types,
                    scratch
                ),
                Err(Incomparable),
                "an unequal pair before the callable does not decide"
            );
            let numbers =
                list(&[Holding::Number(1.0)]).retyped(writer, types.list(KType::NUMBER), types);
            let bools =
                list(&[Holding::Bool(true)]).retyped(writer, types.list(KType::BOOL), types);
            assert_eq!(
                numbers.equals(&bools, types, scratch),
                Ok(false),
                "unrelated container types are unequal without descending"
            );
        })
    });
}

#[test]
fn circular_values_compare_as_a_bisimulation() {
    use super::{Holding, ring};
    with_fixture(|fixture| {
        let (types, scratch, symbols) = (fixture.types, fixture.scratch(), fixture.symbols);
        let next = BinderSymbol::declared("next", symbols).unwrap();
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let equal = |left: Holding<'_, '_>, right: Holding<'_, '_>| {
                left.equals(&right, types, scratch).expect("no function")
            };
            let one = |cell| ring(fixture, writer, KType::STR, &[cell])[0];
            let (first, second) = (one(None), one(None));
            assert!(first != second, "two knots");
            assert!(equal(Holding::Knotted(first), Holding::Knotted(second)));
            let pair = ring(fixture, writer, KType::STR, &[None, None]);
            assert!(
                equal(Holding::Knotted(pair[0]), Holding::Knotted(first)),
                "a two-member ring unrolls to the one-member ring"
            );
            assert!(!equal(
                Holding::Knotted(one(Some(Holding::Number(1.0)))),
                Holding::Knotted(one(Some(Holding::Number(2.0)))),
            ));

            let tagged =
                |payload| Holding::Tagged(crate::values::Tagged::hold(writer, payload, KType::STR));
            let record = |cell| {
                Holding::Record(crate::values::Record::new(
                    writer,
                    &[(next, cell)],
                    types,
                    scratch,
                ))
            };
            let finite = tagged(record(tagged(record(Holding::Null))));
            assert!(!equal(Holding::Knotted(first), finite));
            assert!(!equal(finite, Holding::Knotted(first)));

            let nodes = super::tie(writer, 2, |index, edges| {
                if index == 0 {
                    crate::values::Circular::List(crate::values::List::linked(
                        writer,
                        &[super::Link::Edge(edges[1])],
                        types.list(KType::STR),
                    ))
                } else {
                    crate::values::Circular::Tagged(crate::values::Tagged::linked(
                        writer,
                        super::Link::Edge(edges[0]),
                        KType::STR,
                    ))
                }
            });
            let bools = Holding::List(crate::values::List::new(
                writer,
                [Holding::Bool(true)].into_iter(),
                types,
                scratch,
            ));
            assert!(
                !equal(Holding::Knotted(nodes[0]), bools),
                "a list node and a list of an unrelated type are unequal without descending"
            );
        })
    });
}
