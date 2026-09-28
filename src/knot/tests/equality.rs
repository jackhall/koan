//! A function compares by the `FN` written and its captures, and renders as its type; a builtin
//! compares by its record; a module is incomparable; a knot's data nodes compare as a bisimulation and render with a label wherever a
//! cycle closes.

use crate::type_lattice::{KType, display_name};
use crate::values::{Incomparable, List, Value};

use super::super::builtin;
use super::{bound, pin, with_fixture};

const RING: &str =
    "NEWTYPE Ring = :{next :Ring}\nLET a = (Ring {next = b})\nLET b = (Ring {next = a})";

#[test]
fn a_function_compares_by_the_fn_written_and_its_captures() {
    with_fixture(|fixture| {
        let lines = fixture.parse(
            "LET k = 1\n\
             LET f = (FN :{x :Number} -> Number = #(k))\n\
             LET g = (FN :{x :Number} -> Number = #(k))",
        );
        let (types, scratch) = (fixture.types, fixture.scratch());
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let activation = fixture.run(writer, &lines, &[]);
            let f = bound(fixture, activation, "f");
            let one = Value::Number(1.0);
            let list = |cell| Value::List(List::new(writer, [cell].into_iter(), types, scratch));
            assert_eq!(f.equals(&f, types, scratch), Ok(true));
            assert_eq!(
                f.equals(&bound(fixture, activation, "g"), types, scratch),
                Ok(false),
                "the same text at another site is another function"
            );
            assert_eq!(f.equals(&one, types, scratch), Ok(false));
            assert_eq!(one.equals(&f, types, scratch), Ok(false));
            assert_eq!(list(f).equals(&list(f), types, scratch), Ok(true));
            assert_eq!(
                list(f).equals(&list(one), types, scratch),
                Ok(false),
                "a list of functions and a list of numbers are unrelated, so nothing is compared"
            );
        });
    });
}

#[test]
fn a_callable_renders_as_its_type() {
    with_fixture(|fixture| {
        let lines = fixture.parse("LET k = 7\nLET f = (FN :{x :Number} -> Number = #(k))");
        let (types, symbols, scratch) = (fixture.types, fixture.symbols, fixture.scratch());
        fixture.in_cell(pin, |context| {
            let activation = fixture.run(context.writer(), &lines, &[]);
            let f = bound(fixture, activation, "f");
            let mut rendered = String::new();
            f.render(&mut rendered, types, symbols, scratch).unwrap();
            assert_eq!(
                rendered,
                display_name(f.ktype(), types, symbols).to_string()
            );
            assert_eq!(rendered, ":(FN :{x :Number} -> Number)");
        });
    });
}

#[test]
fn two_rings_from_two_programs_are_equal() {
    with_fixture(|fixture| {
        let (types, scratch) = (fixture.types, fixture.scratch());
        let (pair, single) = (
            fixture.parse(RING),
            fixture.parse("NEWTYPE Ring = :{next :Ring}\nLET a = (Ring {next = a})"),
        );
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let first = fixture.run(writer, &pair, &[]);
            let second = fixture.run(writer, &pair, &[]);
            let lone = fixture.run(writer, &single, &[]);
            let (a, other) = (bound(fixture, first, "a"), bound(fixture, second, "a"));
            assert!(
                a.as_circular().unwrap().0.member().knot()
                    != other.as_circular().unwrap().0.member().knot()
            );
            assert_eq!(a.equals(&other, types, scratch), Ok(true));
            assert_eq!(
                a.equals(&bound(fixture, lone, "a"), types, scratch),
                Ok(true),
                "a two-member ring unrolls to the same infinite value as a self-reference"
            );
        });
    });
}

#[test]
fn a_list_node_holding_a_function_equals_itself() {
    with_fixture(|fixture| {
        let lines = fixture.parse("LET a = [f]\nLET f = (FN :{} -> Any = #(a))");
        let (types, scratch) = (fixture.types, fixture.scratch());
        fixture.in_cell(pin, |context| {
            let activation = fixture.run(context.writer(), &lines, &[]);
            let a = bound(fixture, activation, "a");
            assert_eq!(
                a.equals(&a, types, scratch),
                Ok(true),
                "the function's capture closes the cycle back through the list"
            );
        });
    });
}

#[test]
fn a_module_is_incomparable() {
    with_fixture(|fixture| {
        let lines = fixture.parse("MODULE m = (LET x = 1)");
        let (types, scratch) = (fixture.types, fixture.scratch());
        fixture.in_cell(pin, |context| {
            let activation = fixture.run(context.writer(), &lines, &[]);
            let m = bound(fixture, activation, "m");
            assert_eq!(m.equals(&m, types, scratch), Err(Incomparable));
            assert_eq!(
                m.equals(&crate::knot::KValue::Number(1.0), types, scratch),
                Err(Incomparable)
            );
        });
    });
}

#[test]
fn a_ring_renders_with_a_label_where_it_closes() {
    with_fixture(|fixture| {
        let (types, symbols, scratch) = (fixture.types, fixture.symbols, fixture.scratch());
        for (source, expected) in [
            (
                "NEWTYPE Ring = :{next :Ring}\nLET a = (Ring {next = a})",
                "@0 = Ring({next = @0})",
            ),
            (RING, "@0 = Ring({next = Ring({next = @0})})"),
        ] {
            let lines = fixture.parse(source);
            fixture.in_cell(pin, |context| {
                let activation = fixture.run(context.writer(), &lines, &[]);
                let mut rendered = String::new();
                bound(fixture, activation, "a")
                    .render(&mut rendered, types, symbols, scratch)
                    .unwrap();
                assert_eq!(rendered, expected);
            });
        }
    });
}

#[test]
fn a_builtin_equals_only_itself() {
    with_fixture(|fixture| {
        let (types, scratch) = (fixture.types, fixture.scratch());
        let writer = fixture.program.writer();
        let first = Value::Knotted(builtin(writer, KType::NUMBER, 0));
        let twin = Value::Knotted(builtin(writer, KType::NUMBER, 0));
        assert!(
            first.as_callable().is_some(),
            "a builtin calls as a function"
        );
        assert_eq!(first.equals(&first, types, scratch), Ok(true));
        assert_eq!(
            first.equals(&twin, types, scratch),
            Ok(false),
            "another record of the same overload is another builtin"
        );
    });
}
