//! Fixed activation facts: the layout, the `Empty` invariant, and the builtin table.

use crate::memory::SlotConflict;
use crate::parse::BinderSymbol;
use crate::scope::{Activation, Binding, Builtins, ClosureBindings, Shape};
use crate::values::Value;

use super::{BUILTIN_TYPES, BUILTIN_VALUES, builtins, type_name, value_name, with_fixture};

fn assert_copy<T: Copy>() {}

#[test]
fn an_activation_is_a_copy_of_its_bytes_and_starts_empty() {
    assert_copy::<Activation<'static, 'static>>();
    assert_eq!(size_of::<Activation<'static, 'static>>(), 56);
    with_fixture(|fixture| {
        let lines = fixture.parse("LET a = 1\nLET b = a");
        fixture.in_cell(|writer, handles| {
            let table = builtins(fixture, writer);
            let shape = Shape::of_program(fixture.program, &lines, table, fixture.scratch()).unwrap();
            let activation =
                Activation::new(writer, shape, ClosureBindings::EMPTY, table, None);
            let b_reads_a = shape.mentions()[0];
            let (a, _) = shape.slot(BinderSymbol::Value(value_name("a", fixture.labels))).unwrap();
            activation.claim(a, handles[0]).unwrap();
            assert!(matches!(activation.read(b_reads_a.coordinate), Binding::Pending(handle) if handle == handles[0]));
            let copy = activation;
            copy.bind(a, Value::Number(7.0)).unwrap();
            assert!(matches!(activation.read(b_reads_a.coordinate), Binding::Bound(Value::Number(n)) if n == 7.0));
            assert_eq!(activation.bind(a, Value::Null), Err(SlotConflict::Bound));
        });
    });
}

#[test]
#[should_panic(expected = "a slot visible to a running reader is never empty")]
fn reading_an_unclaimed_slot_breaks_the_scheduler_invariant() {
    with_fixture(|fixture| {
        let lines = fixture.parse("LET a = 1\nLET b = a");
        fixture.in_cell(|writer, _| {
            let shape =
                Shape::of_program(fixture.program, &lines, Builtins::EMPTY, fixture.scratch())
                    .unwrap();
            let activation =
                Activation::new(writer, shape, ClosureBindings::EMPTY, Builtins::EMPTY, None);
            activation.read(shape.mentions()[0].coordinate);
        });
    });
}

#[test]
fn the_builtin_table_sorts_each_channel_and_counts_types_after_values() {
    with_fixture(|fixture| {
        fixture.in_cell(|writer, _| {
            let table = builtins(fixture, writer);
            assert_eq!(table.len(), BUILTIN_VALUES.len() + BUILTIN_TYPES.len());
            let origin = table.value(value_name("origin", fixture.labels)).unwrap();
            assert!(matches!(table.get(origin), Value::Number(n) if n == 0.0));
            for name in BUILTIN_TYPES {
                let index = table.ty(type_name(name, fixture.labels)).unwrap();
                assert!(index.index() >= BUILTIN_VALUES.len());
                assert!(matches!(table.get(index), Value::Type(_)));
            }
            assert!(table.value(value_name("nowhere", fixture.labels)).is_none());
        });
    });
}
