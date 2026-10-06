//! Content digests: equal content digests alike wherever it was laid down, a container's contents
//! are memoized where it is built and shared by a retype, and a literal's digest is known without
//! laying it down.

use proptest::prelude::*;
use proptest::test_runner::{Config as ProptestConfig, TestRunner};

use crate::symbols::BinderSymbol;
use crate::type_lattice::KType;
use crate::values::{Key, literal_digest, record_type};

use super::{Dict, List, Record, Value, pin, text, with_fixture};

#[test]
fn equal_content_digests_alike_in_two_cells() {
    with_fixture(|fixture| {
        let (types, scratch) = (fixture.types, fixture.scratch());
        let build = || {
            fixture.in_cell(pin, |context| {
                let writer = context.writer();
                let cells = [Value::Number(1.0), text(writer, "a"), Value::Null];
                Value::List(List::new(writer, cells.into_iter(), types, scratch)).digest()
            })
        };
        assert_eq!(build(), build());
    });
}

#[test]
fn a_dict_digests_in_key_order_and_a_record_blind_to_field_order() {
    with_fixture(|fixture| {
        let (types, scratch) = (fixture.types, fixture.scratch());
        let name = |text| BinderSymbol::declared(text, fixture.symbols).unwrap();
        let (x, y) = (name("x"), name("y"));
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let (one, two) = (Value::Number(1.0), Value::Number(2.0));
            let (a, b) = (Key::str("a"), Key::str("b"));
            let ab = Dict::new(writer, &[(a, one), (b, two)], types, scratch);
            let ba = Dict::new(writer, &[(b, two), (a, one)], types, scratch);
            assert_eq!(Value::Dict(ab).digest(), Value::Dict(ba).digest());
            let swapped = Dict::new(writer, &[(a, two), (b, one)], types, scratch);
            assert_ne!(Value::Dict(ab).digest(), Value::Dict(swapped).digest());

            let xy = Record::new(writer, &[(x, one), (y, two)], types, scratch);
            let yx = Record::new(writer, &[(y, two), (x, one)], types, scratch);
            assert_eq!(Value::Record(xy).digest(), Value::Record(yx).digest());
        })
    });
}

/// A retype shares its value's contents, so it digests at its new type with no walk; a cell the new
/// type hides still counts.
#[test]
fn a_retype_changes_the_digest_and_hidden_cells_count() {
    with_fixture(|fixture| {
        let (types, scratch) = (fixture.types, fixture.scratch());
        let name = |text| BinderSymbol::declared(text, fixture.symbols).unwrap();
        let (x, y) = (name("x"), name("y"));
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let wide = Record::new(
                writer,
                &[(x, Value::Number(1.0)), (y, Value::Number(2.0))],
                types,
                scratch,
            );
            let only_x = record_type(types, scratch, [(x, KType::NUMBER)].into_iter());
            let narrowed = wide.with_type(writer, only_x);
            assert_ne!(
                Value::Record(wide).digest(),
                Value::Record(narrowed).digest()
            );
            let narrow = Record::new(writer, &[(x, Value::Number(1.0))], types, scratch);
            assert_eq!(narrow.ktype(), narrowed.ktype());
            assert_ne!(
                Value::Record(narrow).digest(),
                Value::Record(narrowed).digest(),
                "the hidden `y` is part of what the retyped record holds"
            );
        })
    });
}

#[test]
fn one_string_literal_digests_alike_from_two_sites() {
    with_fixture(|fixture| {
        let (types, scratch) = (fixture.types, fixture.scratch());
        let (first, second) = (fixture.part("\"ab\""), fixture.part("\"ab\""));
        assert_eq!(
            literal_digest(&first, types, scratch),
            literal_digest(&second, types, scratch)
        );
        assert!(literal_digest(&fixture.part("[1 x]"), types, scratch).is_none());
    });
}

/// A scalar literal's spelling: a small number, a short string, a bool or `null`.
fn scalar() -> BoxedStrategy<String> {
    prop_oneof![
        (-3i32..3).prop_map(|number| number.to_string()),
        "[a-c]{0,2}".prop_map(|text| format!("\"{text}\"")),
        Just("true".to_string()),
        Just("false".to_string()),
        Just("null".to_string()),
    ]
    .boxed()
}

/// A literal's spelling: a scalar, or a list, record or dict of literals, a dict keyed by scalars
/// a key may be.
fn literal() -> BoxedStrategy<String> {
    let key = prop_oneof![
        (-3i32..3).prop_map(|number| number.to_string()),
        "[a-c]{0,2}".prop_map(|text| format!("\"{text}\"")),
        Just("true".to_string()),
    ];
    scalar()
        .prop_recursive(3, 16, 3, move |inner| {
            prop_oneof![
                prop::collection::vec(inner.clone(), 0..3)
                    .prop_map(|items| format!("[{}]", items.join(" "))),
                prop::collection::vec(inner.clone(), 1..3).prop_map(|values| {
                    let fields: Vec<String> = ["x", "y", "z"]
                        .iter()
                        .zip(&values)
                        .map(|(name, value)| format!("{name} = {value}"))
                        .collect();
                    format!("{{{}}}", fields.join(", "))
                }),
                prop::collection::vec((key.clone(), inner), 1..3).prop_map(|pairs| {
                    let entries: Vec<String> = pairs
                        .iter()
                        .map(|(key, value)| format!("{key}: {value}"))
                        .collect();
                    format!("{{{}}}", entries.join(", "))
                }),
            ]
        })
        .boxed()
}

/// The law `literal_digest` keeps: a literal digests as the value `lower_part` lays down for it.
#[test]
fn a_literal_digests_as_the_value_it_lowers_to() {
    with_fixture(|fixture| {
        let (types, scratch) = (fixture.types, fixture.scratch());
        TestRunner::new(ProptestConfig::default())
            .run(&literal(), |source| {
                let part = fixture.part(&source);
                let known = literal_digest(&part, types, scratch);
                let lowered = fixture.in_cell(pin, |context| {
                    Value::lower_part(context.writer(), &part, types, scratch)
                        .map(|value: Value<'_>| value.digest())
                });
                prop_assert_eq!(known, lowered, "`{}`", source);
                Ok(())
            })
            .unwrap_or_else(|failure| panic!("{failure}"));
    });
}
