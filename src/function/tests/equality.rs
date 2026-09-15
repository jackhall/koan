//! A callable compares as an error and renders as its type; a knot's data nodes compare as a
//! bisimulation and render with a label wherever a cycle closes.

use crate::type_lattice::display_name;
use crate::values::{Incomparable, List, Value};

use super::{bound, pin, with_fixture};

const RING: &str = "LET a = (Ring {next = b})\nLET b = (Ring {next = a})";

#[test]
fn a_comparison_reaching_a_callable_is_an_error() {
    with_fixture(|fixture| {
        let lines = fixture.parse("LET f = (FN :{x :Number} -> Number = (x))");
        let (types, scratch) = (fixture.types, fixture.scratch());
        fixture.in_cell(pin, |context, binder| {
            let writer = context.writer();
            let activation = fixture.run(writer, &lines, binder, &[]);
            let f = bound(fixture, activation, "f");
            let one = Value::Number(1.0);
            let list = |cell| Value::List(List::new(writer, [cell].into_iter(), types, scratch));
            assert_eq!(f.equals(&f, types, scratch), Err(Incomparable));
            assert_eq!(f.equals(&one, types, scratch), Err(Incomparable));
            assert_eq!(one.equals(&f, types, scratch), Err(Incomparable));
            assert_eq!(list(f).equals(&list(f), types, scratch), Err(Incomparable));
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
        let lines = fixture.parse("LET k = 7\nLET f = (FN :{x :Number} -> Number = (k))");
        let (types, labels, scratch) = (fixture.types, fixture.labels, fixture.scratch());
        fixture.in_cell(pin, |context, binder| {
            let activation = fixture.run(context.writer(), &lines, binder, &[]);
            let f = bound(fixture, activation, "f");
            let mut rendered = String::new();
            f.render(&mut rendered, types, labels, scratch).unwrap();
            assert_eq!(rendered, display_name(f.ktype(), types, labels).to_string());
            assert_eq!(rendered, ":(FN :{x :Number} -> Number)");
        });
    });
}

#[test]
fn two_rings_from_two_programs_are_equal() {
    with_fixture(|fixture| {
        let ring = fixture.ring_type("Ring", "next");
        let (types, scratch) = (fixture.types, fixture.scratch());
        let (pair, single) = (
            fixture.parse(RING),
            fixture.parse("LET a = (Ring {next = a})"),
        );
        fixture.in_cell(pin, |context, binder| {
            let writer = context.writer();
            let nominals = [("Ring", ring)];
            let first = fixture.run_with(writer, &pair, binder, &[], &nominals);
            let second = fixture.run_with(writer, &pair, binder, &[], &nominals);
            let lone = fixture.run_with(writer, &single, binder, &[], &nominals);
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
fn a_list_node_holding_a_function_is_incomparable() {
    with_fixture(|fixture| {
        let lines = fixture.parse("LET a = [f]\nLET f = (FN :{} -> Any = (a))");
        let (types, scratch) = (fixture.types, fixture.scratch());
        fixture.in_cell(pin, |context, binder| {
            let activation = fixture.run(context.writer(), &lines, binder, &[]);
            let a = bound(fixture, activation, "a");
            assert_eq!(a.equals(&a, types, scratch), Err(Incomparable));
        });
    });
}

#[test]
fn a_ring_renders_with_a_label_where_it_closes() {
    with_fixture(|fixture| {
        let ring = fixture.ring_type("Ring", "next");
        let (types, labels, scratch) = (fixture.types, fixture.labels, fixture.scratch());
        for (source, expected) in [
            ("LET a = (Ring {next = a})", "@0 = Ring({next = @0})"),
            (RING, "@0 = Ring({next = Ring({next = @0})})"),
        ] {
            let lines = fixture.parse(source);
            fixture.in_cell(pin, |context, binder| {
                let activation =
                    fixture.run_with(context.writer(), &lines, binder, &[], &[("Ring", ring)]);
                let mut rendered = String::new();
                bound(fixture, activation, "a")
                    .render(&mut rendered, types, labels, scratch)
                    .unwrap();
                assert_eq!(rendered, expected);
            });
        }
    });
}
