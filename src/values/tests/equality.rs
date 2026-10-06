//! Structural equality: IEEE numbers, nominal identity first, containers gated on related types,
//! quotes compared as syntax, functions by identity.

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
            let equal = |left: &Value<'_>, right: &Value<'_>| {
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
            let equal = |left: Value<'_>, right: Value<'_>| {
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
                one_two.retyped(writer, KType::LIST_OF_ANY, types, scratch)
            ));
            let empty = Value::List(List::new(writer, [].into_iter(), types, scratch));
            let empty_strings = empty.retyped(writer, types.list(KType::STR), types, scratch);
            let empty_numbers = empty.retyped(writer, types.list(KType::NUMBER), types, scratch);
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
fn quotes_compare_as_syntax_marks_included() {
    use super::{Stand, quote};
    use crate::values::Value as Holding;
    with_fixture(|fixture| {
        let (types, scratch) = (fixture.types, fixture.scratch());
        let code = |source: &str| Holding::Knotted(Stand::Code(quote(fixture, source)));
        let equal = |left: &str, right: &str| {
            code(left)
                .equals(&code(right), types, scratch)
                .expect("code is comparable")
        };
        assert!(equal("#(a [1 2] {x = \"s\"})", "#(a [1 2] {x = \"s\"})"));
        assert!(!equal("#(a [1 2])", "#(a [2 1])"));
        assert!(!equal("#(a)", "#(a b)"));
        assert!(equal("#($a PLUS \\(PRINT c))", "#($a PLUS \\(PRINT c))"));
        assert!(!equal("#($a)", "#(a)"), "a mark is syntax");
        assert!(!equal("#($a)", "#(\\a)"), "and so is which mark");
    });
}

#[test]
fn a_function_compares_by_identity_and_a_barrier_is_incomparable() {
    use super::Stand;
    use crate::values::{Incomparable, Value as Holding};
    with_fixture(|fixture| {
        let (types, scratch) = (fixture.types, fixture.scratch());
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let (one, other) = (
                Holding::Knotted(Stand::Function(1)),
                Holding::Knotted(Stand::Function(2)),
            );
            let barrier = Holding::Knotted(Stand::Barrier);
            let list = |items: &[_]| {
                Holding::List(crate::values::List::new(
                    writer,
                    items.iter().copied(),
                    types,
                    scratch,
                ))
            };
            assert_eq!(one.equals(&one, types, scratch), Ok(true));
            assert_eq!(one.equals(&other, types, scratch), Ok(false));
            assert_eq!(barrier.equals(&barrier, types, scratch), Err(Incomparable));
            assert_eq!(
                Holding::<Stand>::Number(1.0).equals(&barrier, types, scratch),
                Err(Incomparable)
            );
            assert_eq!(
                list(&[Holding::Number(1.0), barrier]).equals(
                    &list(&[Holding::Number(2.0), barrier]),
                    types,
                    scratch
                ),
                Err(Incomparable),
                "an unequal pair before the barrier does not decide"
            );
            let numbers = list(&[Holding::Number(1.0)]).retyped(
                writer,
                types.list(KType::NUMBER),
                types,
                scratch,
            );
            let bools = list(&[Holding::Bool(true)]).retyped(
                writer,
                types.list(KType::BOOL),
                types,
                scratch,
            );
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
            let equal = |left: Holding<'_>, right: Holding<'_>| {
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

#[test]
fn a_seal_its_bound_reveals_is_read_through_and_any_other_stays() {
    use crate::type_lattice::ContentKey;
    use crate::values::KeyRejected;
    with_fixture(|fixture| {
        let (types, scratch, symbols) = (fixture.types, fixture.scratch(), fixture.symbols);
        let carrier = crate::symbols::TypeSymbol::declared("Carrier", symbols).unwrap();
        // Views of two contents key their carriers apart, so two carriers of one bound are two
        // identities.
        let keys = std::cell::Cell::new(0);
        let mint = |bound| {
            keys.set(keys.get() + 1);
            types.carrier(carrier, bound, ContentKey(keys.get()))
        };
        let number_or_str = types.union_of(scratch, &[KType::NUMBER, KType::STR]);
        let (by_number, by_number_again) = (mint(KType::NUMBER), mint(KType::NUMBER));
        let (by_value, by_either) = (mint(KType::ANY_VALUE), mint(number_or_str));
        let distance = fixture.newtype("Distance", KType::NUMBER);
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let five = Value::Number(5.0);
            let sealed = |mint| {
                let tagged = Tagged::seal(writer, five, mint, KType::NUMBER, types, scratch);
                Value::Tagged(tagged.expect("5 satisfies its witness"))
            };
            let equal = |left: Value<'_>, right: Value<'_>| {
                left.equals(&right, types, scratch).expect("no callable")
            };
            assert!(equal(sealed(by_number), five));
            assert!(equal(five, sealed(by_number)));
            assert!(equal(sealed(by_number), sealed(by_number_again)));
            let key = Key::of(&sealed(by_number), types, scratch).expect("a revealed number keys");
            assert_eq!(key, Key::number(5.0).unwrap());
            assert!(matches!(
                key.value::<crate::values::Nothing>(),
                Value::Number(5.0)
            ));

            // A bound that does not reveal the payload's kind keeps the seal.
            for mint in [by_value, by_either] {
                assert!(!equal(sealed(mint), five));
                assert_eq!(
                    Key::of(&sealed(mint), types, scratch),
                    Err(KeyRejected::NotAScalar(mint))
                );
            }
            // A newtype is no seal: it is nominal whatever it wraps.
            let wrapped = Value::Tagged(Tagged::hold(writer, five, distance));
            assert!(!equal(wrapped, five));
        })
    });
}

/// Every quote is one representation, so a seal bounded by any code kind reveals every quote it
/// holds, whatever that quote's own kind; one bounded past `Code` keeps it.
#[test]
fn a_seal_bounded_by_a_code_kind_reveals_every_quote() {
    use super::{Stand, quote};
    use crate::type_lattice::ContentKey;
    use crate::values::Value as Holding;
    with_fixture(|fixture| {
        let (types, scratch, symbols) = (fixture.types, fixture.scratch(), fixture.symbols);
        let carrier = crate::symbols::TypeSymbol::declared("Carrier", symbols).unwrap();
        let keys = std::cell::Cell::new(0);
        let mint = |bound| {
            keys.set(keys.get() + 1);
            types.carrier(carrier, bound, ContentKey(keys.get()))
        };
        let name = quote(fixture, "#(y)");
        let call = quote(fixture, "#(f x)");
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let equal = |left: Holding<'_, Stand<'_>>, right: Holding<'_, Stand<'_>>| {
                left.equals(&right, types, scratch).expect("no callable")
            };
            for node in [name, call] {
                let quote = Holding::Knotted(Stand::Code(node));
                let sealed = |bound| {
                    let tagged = crate::values::Tagged::seal(
                        writer,
                        quote,
                        mint(bound),
                        quote.concrete_ktype(),
                        types,
                        scratch,
                    );
                    Holding::Tagged(tagged.expect("the quote satisfies its witness"))
                };
                assert!(equal(sealed(KType::EXPRESSION), quote), "{quote:?}");
                assert!(equal(sealed(KType::ANY_CODE), quote), "{quote:?}");
                assert!(!equal(sealed(KType::ANY), quote), "{quote:?}");
            }
        })
    });
}
