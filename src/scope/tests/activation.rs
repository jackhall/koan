//! Fixed activation facts: the layout, the empty-slot invariant, and the builtin table.

use crate::memory::SlotConflict;
use crate::scope::{Activation, ActivationView, BodyShape, Builtins};
use crate::symbols::BinderSymbol;
use crate::values::Value;

use super::{BUILTIN_TYPES, BUILTIN_VALUES, builtins, type_name, value_name, with_fixture};

fn assert_copy<T: Copy>() {}

#[test]
fn an_activation_is_a_copy_of_its_bytes_and_starts_empty() {
    assert_copy::<Activation<'static, 'static>>();
    assert_copy::<ActivationView<'static, 'static>>();
    assert_eq!(size_of::<Activation<'static, 'static>>(), 64);
    assert_eq!(size_of::<ActivationView<'static, 'static>>(), 48);
    with_fixture(|fixture| {
        let lines = fixture.parse("LET a = 1\nLET b = a");
        fixture.in_cell(|writer| {
            let table: &Builtins = builtins(fixture, writer);
            let shape =
                BodyShape::of_program(fixture.program, &lines, table, fixture.scratch()).unwrap();
            let activation: Activation = Activation::of_program(writer, shape, table);
            let view = activation.view();
            let b_reads_a = shape.mentions()[0];
            let (a, _) = shape
                .slot(BinderSymbol::Value(value_name("a", fixture.symbols)))
                .unwrap();
            let copy = activation;
            copy.bind(a, Value::Number(7.0)).unwrap();
            assert!(matches!(activation.read(b_reads_a.coordinate), Value::Number(n) if n == 7.0));
            assert!(
                matches!(view.read(b_reads_a.coordinate), Value::Number(n) if n == 7.0),
                "a view taken before the bind sees it"
            );
            assert_eq!(activation.bind(a, Value::Null), Err(SlotConflict));
        });
    });
}

#[test]
#[should_panic(expected = "a slot visible to a running reader is never empty")]
fn reading_an_empty_slot_breaks_the_scheduler_invariant() {
    with_fixture(|fixture| {
        let lines = fixture.parse("LET a = 1\nLET b = a");
        fixture.in_cell(|writer| {
            let table: &Builtins = Builtins::empty();
            let shape =
                BodyShape::of_program(fixture.program, &lines, table, fixture.scratch()).unwrap();
            let activation: Activation = Activation::of_program(writer, shape, table);
            activation.read(shape.mentions()[0].coordinate);
        });
    });
}

#[test]
fn the_builtin_table_sorts_each_channel_and_counts_types_after_values() {
    with_fixture(|fixture| {
        fixture.in_cell(|writer| {
            let table: &Builtins = builtins(fixture, writer);
            assert_eq!(table.len(), BUILTIN_VALUES.len() + BUILTIN_TYPES.len());
            let origin = table
                .lookup(BinderSymbol::Value(value_name("origin", fixture.symbols)))
                .unwrap();
            assert!(matches!(table.get(origin), Value::Number(n) if n == 0.0));
            for name in BUILTIN_TYPES {
                let index = table
                    .lookup(BinderSymbol::Type(type_name(name, fixture.symbols)))
                    .unwrap();
                assert!(index.index() >= BUILTIN_VALUES.len());
                assert!(matches!(table.get(index), Value::Type(_)));
            }
            assert!(
                table
                    .lookup(BinderSymbol::Value(value_name("nowhere", fixture.symbols)))
                    .is_none()
            );
        });
    });
}
