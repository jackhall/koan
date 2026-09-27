//! A quote's code as a knot member: born through the quote door or tied into its binder's knot,
//! compared as a bisimulation that follows its bound names, printed as written, and filled by
//! `USING`.

use std::ptr;

use crate::type_lattice::display_name;
use crate::values::{Knotted as _, Link, Value};

use super::super::{Knotted, using};
use super::{Fixture, bound, pin, with_fixture};

/// The quote bound under `name`.
fn code<'graph, 'cell>(
    fixture: &Fixture<'_, 'graph>,
    activation: &crate::knot::KActivation<'graph, 'cell>,
    name: &str,
) -> Knotted<'graph, 'cell> {
    bound(fixture, activation, name)
        .as_code()
        .unwrap_or_else(|| panic!("`{name}` is bound to code"))
}

fn rendered(fixture: &Fixture<'_, '_>, member: Knotted<'_, '_>) -> String {
    let mut out = String::new();
    Value::Knotted(member)
        .render(&mut out, fixture.types, fixture.symbols, fixture.scratch())
        .unwrap();
    out
}

#[test]
fn a_quote_binds_its_dollar_names_where_it_is_written_and_prints_its_marks() {
    with_fixture(|fixture| {
        let lines = fixture.parse("LET k = 7\nLET q = #($k PLUS hole PLUS \\it)");
        fixture.in_cell(pin, |context| {
            let activation = fixture.run(context.writer(), &lines, &[]);
            let q = code(fixture, activation, "q");
            let node = q.code().expect("a quote's node");
            assert_eq!(q.member().knot().len(), 1, "a quote is a one-node knot");
            assert!(matches!(
                node.bound(),
                [(name, Link::Value(Value::Number(7.0)))] if *name == fixture.name("k")
            ));
            assert!(node.supplied().is_empty());
            assert_eq!(
                display_name(q.ktype(), fixture.types, fixture.symbols).to_string(),
                ":(Expression NEEDING #[it])"
            );
            assert_eq!(
                rendered(fixture, q),
                "$k PLUS hole PLUS \\it",
                "code prints each mark as written, and never a binding"
            );
        });
    });
}

#[test]
fn code_compares_by_its_text_and_the_values_its_names_bind() {
    with_fixture(|fixture| {
        let (types, scratch) = (fixture.types, fixture.scratch());
        let [one, again, two, hole] = [
            "LET a = 1\nLET q = #($a MINUS 1)",
            "LET a = 1\nLET q = #($a MINUS 1)",
            "LET a = 2\nLET q = #($a MINUS 1)",
            "LET a = 1\nLET q = #(a MINUS 1)",
        ]
        .map(|source| fixture.parse(source));
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let [one, again, two, hole] = [&one, &again, &two, &hole]
                .map(|lines| Value::Knotted(code(fixture, fixture.run(writer, lines, &[]), "q")));
            assert_eq!(one.equals(&again, types, scratch), Ok(true));
            assert_eq!(
                one.equals(&two, types, scratch),
                Ok(false),
                "the same text whose `$` name binds another value"
            );
            assert_eq!(
                one.equals(&hole, types, scratch),
                Ok(false),
                "a bound name is not a hole"
            );
        });
    });
}

#[test]
fn a_quote_reading_its_own_binder_is_a_one_node_knot() {
    with_fixture(|fixture| {
        let lines = fixture.parse("LET echo = #(PRINT $echo)");
        let (types, scratch) = (fixture.types, fixture.scratch());
        fixture.in_cell(pin, |context| {
            let activation = fixture.run(context.writer(), &lines, &[]);
            let echo = code(fixture, activation, "echo");
            assert_eq!(echo.member().knot().len(), 1);
            let [(_, Link::Edge(edge))] = echo.code().expect("a quote's node").bound() else {
                panic!("`$echo` is an edge into its own knot");
            };
            assert!(ptr::eq(echo.sibling(*edge).node(), echo.node()));
            assert_eq!(
                Value::Knotted(echo).equals(&Value::Knotted(echo), types, scratch),
                Ok(true)
            );
            assert_eq!(rendered(fixture, echo), "PRINT $echo");
        });
    });
}

#[test]
fn using_fills_the_holes_a_record_names_and_ignores_the_rest() {
    with_fixture(|fixture| {
        let lines = fixture.parse(
            "LET q = #(x PLUS y)\n\
             LET first = {x = 1, z = 3}\n\
             LET second = {x = 5, y = 2}\n\
             LET other = {x = 9}",
        );
        let (types, scratch) = (fixture.types, fixture.scratch());
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let activation = fixture.run(writer, &lines, &[]);
            let q = code(fixture, activation, "q");
            let record = |name| bound(fixture, activation, name);
            let supplied = |member: Knotted<'_, '_>| -> Vec<_> {
                member
                    .code()
                    .expect("a quote's node")
                    .supplied()
                    .iter()
                    .map(|(name, link)| match link {
                        Link::Value(Value::Number(number)) => (*name, *number),
                        _ => panic!("a supplied binding is a number word"),
                    })
                    .collect()
            };
            let (x, y) = (fixture.name("x"), fixture.name("y"));

            let filled = using(writer, q, record("first"), types, scratch);
            assert_eq!(supplied(filled), [(x, 1.0)], "`z` names no hole");
            assert!(supplied(q).is_empty(), "the source code is unchanged");
            assert_eq!(filled.ktype(), q.ktype());

            let both = using(writer, filled, record("second"), types, scratch);
            let mut expected = [(x, 1.0), (y, 2.0)];
            expected.sort_by_key(|(name, _)| *name);
            assert_eq!(
                supplied(both),
                expected,
                "a hole an earlier `USING` filled is never rebound"
            );

            let equal = |left: Knotted<'_, '_>, right: Knotted<'_, '_>| {
                Value::Knotted(left).equals(&Value::Knotted(right), types, scratch)
            };
            let twice = using(writer, q, record("first"), types, scratch);
            assert_eq!(equal(filled, twice), Ok(true));
            let nine = using(writer, q, record("other"), types, scratch);
            assert_eq!(equal(filled, nine), Ok(false));
            assert_eq!(equal(filled, q), Ok(false), "a filled hole is a binding");
        });
    });
}

#[test]
fn using_reads_a_module_by_member_name_and_keeps_refused_code_unchanged() {
    with_fixture(|fixture| {
        let lines = fixture.parse(
            "MODULE m = (LET x = 4)\n\
             LET q = #(x PLUS 1)\n\
             LET bad = #((LET x = 1) (LET x = 2) (PRINT x))",
        );
        let (types, scratch) = (fixture.types, fixture.scratch());
        fixture.in_cell(pin, |context| {
            let writer = context.writer();
            let activation = fixture.run(writer, &lines, &[]);
            let m = bound(fixture, activation, "m");
            let q = using(writer, code(fixture, activation, "q"), m, types, scratch);
            assert!(matches!(
                q.code().expect("a quote's node").supplied(),
                [(_, Link::Value(Value::Number(4.0)))]
            ));
            let bad = code(fixture, activation, "bad");
            assert!(
                bad.code()
                    .expect("a quote's node")
                    .shape()
                    .refusal()
                    .is_some()
            );
            let kept = using(writer, bad, m, types, scratch);
            assert!(
                ptr::eq(kept.node(), bad.node()),
                "a refused code's holes are unknown"
            );
        });
    });
}
