//! `satisfies`: one lattice relation over a value's memoized type, or unification for a slot that
//! reads a quantifier.

use crate::type_lattice::{KKind, KType};
use crate::values::{TypeValue, satisfies};

use super::{List, Tagged, Value, pin, text, with_fixture};

#[test]
fn leaves_any_and_never_answer_by_the_order() {
    with_fixture(|fixture| {
        let (types, scratch) = (fixture.types, fixture.scratch());
        fixture.in_cell(pin, |context| {
            let string = text(context.writer(), "s");
            assert!(satisfies(
                KType::NUMBER,
                &Value::Number(1.0),
                types,
                scratch
            ));
            assert!(!satisfies(KType::NUMBER, &string, types, scratch));
            assert!(satisfies(KType::STR, &string, types, scratch));
            assert!(satisfies(KType::NULL, &Value::Null, types, scratch));
            assert!(satisfies(KType::ANY, &Value::Bool(false), types, scratch));
            assert!(!satisfies(KType::NEVER, &Value::Null, types, scratch));
            let tagged = Tagged::hold(context.writer(), Value::Number(1.0), KType::STR);
            assert!(!satisfies(
                KType::NUMBER,
                &Value::Tagged(tagged),
                types,
                scratch
            ));
        })
    });
}

#[test]
fn a_union_slot_takes_what_any_member_takes() {
    with_fixture(|fixture| {
        let (types, scratch) = (fixture.types, fixture.scratch());
        let union = types.union_of(scratch, &[KType::NUMBER, KType::STR]);
        assert!(satisfies(union, &Value::Number(1.0), types, scratch));
        assert!(!satisfies(union, &Value::Bool(true), types, scratch));
    });
}

#[test]
fn a_list_slot_reads_the_memoized_list_type() {
    with_fixture(|fixture| {
        let (types, scratch) = (fixture.types, fixture.scratch());
        fixture.in_cell(pin, |context| {
            let numbers = [Value::Number(1.0), Value::Number(2.0)];
            let list = Value::List(List::new(
                context.writer(),
                numbers.into_iter(),
                types,
                scratch,
            ));
            assert!(satisfies(types.list(KType::NUMBER), &list, types, scratch));
            assert!(satisfies(KType::LIST_OF_ANY, &list, types, scratch));
            assert!(!satisfies(types.list(KType::STR), &list, types, scratch));
            assert!(!satisfies(
                types.dict(KType::ANY, KType::ANY),
                &list,
                types,
                scratch
            ));
        })
    });
}

#[test]
fn a_kind_slot_takes_a_type_value_of_that_kind() {
    with_fixture(|fixture| {
        let (types, scratch) = (fixture.types, fixture.scratch());
        fixture.in_cell(pin, |context| {
            let number = Value::Type(TypeValue::new(context.writer(), KType::NUMBER, types));
            assert!(satisfies(
                KType::of_kind(KKind::ProperType),
                &number,
                types,
                scratch
            ));
            assert!(satisfies(
                KType::of_kind(KKind::AnyType),
                &number,
                types,
                scratch
            ));
            assert!(!satisfies(
                KType::of_kind(KKind::Signature),
                &number,
                types,
                scratch
            ));
            assert!(!satisfies(
                KType::of_kind(KKind::ProperType),
                &Value::Number(1.0),
                types,
                scratch
            ));
        })
    });
}

#[test]
fn a_quantified_slot_admits_by_unification() {
    with_fixture(|fixture| {
        let (types, scratch) = (fixture.types, fixture.scratch());
        let variable = types.quantified(0, KType::ANY);
        let list_of_variable = types.list(variable);
        fixture.in_cell(pin, |context| {
            let numbers = [Value::Number(1.0)];
            let list = Value::List(List::new(
                context.writer(),
                numbers.into_iter(),
                types,
                scratch,
            ));
            assert!(satisfies(variable, &Value::Number(1.0), types, scratch));
            assert!(satisfies(list_of_variable, &list, types, scratch));
            assert!(!satisfies(
                list_of_variable,
                &Value::Number(1.0),
                types,
                scratch
            ));
            assert!(!satisfies(
                list_of_variable,
                &text(context.writer(), "s"),
                types,
                scratch
            ));
        })
    });
}

#[test]
fn a_circular_value_satisfies_by_its_node_memo() {
    use super::{Holding, ring};
    with_fixture(|fixture| {
        let (types, scratch) = (fixture.types, fixture.scratch());
        let ring_type = fixture.ring_type("Ring", "next");
        fixture.in_cell(pin, |context| {
            let member = Holding::Knotted(ring(fixture, context.writer(), ring_type, &[None])[0]);
            assert!(satisfies(ring_type, &member, types, scratch));
            assert!(satisfies(KType::ANY, &member, types, scratch));
            assert!(!satisfies(KType::NUMBER, &member, types, scratch));
        })
    });
}

#[test]
fn a_value_a_type_and_a_quote_each_satisfy_their_family_alone() {
    use super::{Stand, quote};
    use crate::values::Value as Holding;
    with_fixture(|fixture| {
        let (types, scratch) = (fixture.types, fixture.scratch());
        let quote = quote(fixture, "#(a)");
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let number = Holding::Number(1.0);
            let type_value = Holding::Type(TypeValue::new(writer, KType::NUMBER, types));
            let quote = Holding::Knotted(Stand::Code(quote));
            let families = [
                (&number, [KType::ANY_VALUE, KType::NUMBER]),
                (&type_value, [KType::ANY_TYPE, KType::PROPER_TYPE]),
                (&quote, [KType::ANY_CODE, KType::EXPRESSION]),
            ];
            for (value, own) in &families {
                assert!(satisfies(KType::ANY, value, types, scratch));
                for top in [KType::ANY_VALUE, KType::ANY_TYPE, KType::ANY_CODE] {
                    assert_eq!(
                        satisfies(top, value, types, scratch),
                        top == own[0],
                        "{value:?} against {top:?}"
                    );
                }
                assert!(satisfies(own[1], value, types, scratch));
            }
        })
    });
}

/// A quote satisfies its own code kind and every kind above it in the code family's tree, and no
/// other.
#[test]
fn a_quote_satisfies_its_own_kind_and_every_kind_above_it() {
    use super::{Stand, quote};
    use crate::values::Value as Holding;
    with_fixture(|fixture| {
        let (types, scratch) = (fixture.types, fixture.scratch());
        let kinds = [
            KType::IDENTIFIER,
            KType::TYPE_NAME_TOKEN,
            KType::NAME,
            KType::KEYWORD,
            KType::SYMBOL,
            KType::LITERAL,
            KType::BINDER,
            KType::DECLARATION,
            KType::EXPRESSION,
            KType::BLOCK,
            KType::ANY_CODE,
        ];
        let cases: [(&str, &[KType]); 7] = [
            (
                "#(y)",
                &[
                    KType::IDENTIFIER,
                    KType::NAME,
                    KType::SYMBOL,
                    KType::EXPRESSION,
                    KType::BLOCK,
                    KType::ANY_CODE,
                ],
            ),
            (
                "#(+)",
                &[
                    KType::KEYWORD,
                    KType::SYMBOL,
                    KType::EXPRESSION,
                    KType::BLOCK,
                    KType::ANY_CODE,
                ],
            ),
            (
                "#((y))",
                &[KType::EXPRESSION, KType::BLOCK, KType::ANY_CODE],
            ),
            (
                "#(LET x = 1)",
                &[
                    KType::BINDER,
                    KType::DECLARATION,
                    KType::EXPRESSION,
                    KType::BLOCK,
                    KType::ANY_CODE,
                ],
            ),
            (
                "#(VAL x :Str)",
                &[
                    KType::DECLARATION,
                    KType::EXPRESSION,
                    KType::BLOCK,
                    KType::ANY_CODE,
                ],
            ),
            (
                "#(TYPE Carrier)",
                &[
                    KType::DECLARATION,
                    KType::EXPRESSION,
                    KType::BLOCK,
                    KType::ANY_CODE,
                ],
            ),
            ("#((f x) (g y))", &[KType::BLOCK, KType::ANY_CODE]),
        ];
        let parts: Vec<_> = cases
            .iter()
            .map(|(source, _)| quote(fixture, source))
            .collect();
        fixture.in_cell(pin, |_| {
            for ((source, above), part) in cases.iter().zip(&parts) {
                let quote = Holding::Knotted(Stand::Code(*part));
                for kind in kinds {
                    assert_eq!(
                        satisfies(kind, &quote, types, scratch),
                        above.contains(&kind),
                        "{source} against {kind:?}"
                    );
                }
            }
        })
    });
}

/// A name slot takes no string, and a code slot no number: the table half of `ATTR`'s and `EVAL`'s
/// slot types.
#[test]
fn a_code_slot_takes_no_value() {
    with_fixture(|fixture| {
        let (types, scratch) = (fixture.types, fixture.scratch());
        fixture.in_cell(pin, |context| {
            let string = text(context.writer(), "y");
            assert!(!satisfies(KType::NAME, &string, types, scratch));
            assert!(!satisfies(
                KType::ANY_CODE,
                &Value::Number(1.0),
                types,
                scratch
            ));
        })
    });
}
