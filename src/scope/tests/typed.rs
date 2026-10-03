//! The type expressions a shape records for the load pass: a type in value position, a type part
//! of an expression shape that births no callable, and each `MATCH … WITH` guard — and nothing a
//! callable or a binder is typed with, nor a type nested in a recorded one.

use crate::parse::ExpressionPart;
use crate::scope::{BodyShape, Builtins, Static};

use super::{builtins, with_fixture};

#[test]
fn the_builder_records_each_type_expression_once() {
    let source = "LET t = :(LIST OF Number)\n\
                  LET Alias = :(LIST OF Str)\n\
                  NEWTYPE Wrapped = :{v :(LIST OF Number)}\n\
                  LET f = (FN :{x :Number} -> :(LIST OF Number) = #(x))\n\
                  MATCH 1 -> :Number WITH #{Number: (1), :(Str): (2), _: (3)}";
    with_fixture(|fixture| {
        let lines = fixture.parse(source);
        fixture.in_cell(|writer| {
            let table: &Builtins = builtins(fixture, writer);
            let shape = BodyShape::of_program(
                fixture.program,
                &lines,
                table,
                fixture.types,
                fixture.symbols,
                fixture.scratch(),
            )
            .expect("the program shapes");
            let recorded = shape.type_expressions();
            let per_statement: Vec<usize> = (0..5)
                .map(|statement| {
                    recorded
                        .iter()
                        .filter(|expression| expression.statement == statement)
                        .count()
                })
                .collect();
            assert_eq!(per_statement, [1, 0, 0, 0, 3]);
            assert!(
                recorded
                    .iter()
                    .all(|expression| { matches!(expression.typed(), Static::Unknown) })
            );
            let mut guards: Vec<_> = recorded
                .iter()
                .filter_map(|expression| Some((expression.guard?, expression.part)))
                .collect();
            guards.sort_by_key(|((_, index), _)| *index);
            let [((first, 0), one), ((second, 1), other)] = guards[..] else {
                panic!("two guards, in written order: {}", guards.len())
            };
            assert_eq!(first, second, "one arm set");
            assert!(matches!(one, ExpressionPart::QuotedExpression(_)));
            assert!(matches!(other, ExpressionPart::QuotedExpression(_)));
            let callable = shape
                .nested_shapes()
                .iter()
                .find(|(_, nested)| nested.form().is_some())
                .expect("the lambda's body");
            assert!(callable.1.type_expressions().is_empty());
        })
    });
}
