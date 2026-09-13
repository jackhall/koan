//! `satisfies`: one lattice relation over a value's memoized type, or unification for a slot that
//! reads a quantifier.

use crate::type_lattice::{KKind, KType};
use crate::values::{List, Tagged, TypeValue, Value, satisfies, text};

use super::{pin, with_fixture};

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
