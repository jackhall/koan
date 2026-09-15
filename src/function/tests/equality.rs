//! A callable compares as an error and renders as its type.

use crate::type_lattice::display_name;
use crate::values::{Incomparable, List, Value};

use super::{bound, pin, with_fixture};

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
