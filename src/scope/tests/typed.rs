//! The type expressions a shape records for the load pass: a type in value position, a type part
//! of an expression shape that births no callable, and each `MATCH … WITH` guard — and nothing a
//! callable or a binder is typed with, nor a type nested in a recorded one — and each path a type
//! spells, wherever it sits.

use crate::parse::ExpressionPart;
use crate::scope::{
    BodyShape, Builtins, PathEntry, Resolution, Settled, ShapeFacts, Site, Static, StaticType,
    Statics, Tree, source_of,
};

use super::{builtins, with_fixture};

// The facts vocabulary names a body by its segment and holds no reference into program storage, so
// each type is covariant in its one lifetime: one laid in program storage reads as one laid in any
// shorter-lived region.

#[allow(dead_code)]
fn a_static_type_shortens<'short, 'long: 'short>(x: StaticType<'long>) -> StaticType<'short> {
    x
}

#[allow(dead_code)]
fn statics_shorten<'short, 'long: 'short>(x: Statics<'long>) -> Statics<'short> {
    x
}

#[allow(dead_code)]
fn shape_facts_shorten<'short, 'long: 'short>(x: ShapeFacts<'long>) -> ShapeFacts<'short> {
    x
}

#[allow(dead_code)]
fn a_resolution_shortens<'short, 'long: 'short>(x: Resolution<'long>) -> Resolution<'short> {
    x
}

#[allow(dead_code)]
fn settled_shortens<'short, 'long: 'short>(x: Settled<'long>) -> Settled<'short> {
    x
}

#[allow(dead_code)]
fn a_tree_shortens<'short, 'long: 'short>(x: Tree<'long>) -> Tree<'short> {
    x
}

#[allow(dead_code)]
fn a_path_entry_shortens<'short, 'long: 'short>(x: PathEntry<'long>) -> PathEntry<'short> {
    x
}

#[test]
fn the_builder_records_each_type_expression_once() {
    let source = "LET t = :(LIST OF Number)\n\
                  LET Alias = :(LIST OF Str)\n\
                  NEWTYPE Wrapped = :{v :(LIST OF Number)}\n\
                  LET f = (FN :{x :Number} -> :(LIST OF Number) = #(x))\n\
                  MATCH 1 -> :Number WITH #{Number: (1), :(Str): (2), _: (3)}";
    with_fixture(|fixture| {
        let lines = fixture.parse(source);
        fixture.in_cell(|writer, _| {
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

/// Every path a type spells — in a type binder, an annotation, a signature and a type in value
/// position — is recorded on the shape whose types spell it, as met; a projection is not.
#[test]
fn the_builder_records_each_path_a_type_spells() {
    let source = "MODULE ints = (LET Carrier = Number)\n\
                  LET Alias = :(ints.Carrier)\n\
                  LET x :(ints.Carrier) = 1\n\
                  LET f = (FN :{y :(ints.Carrier)} -> Any = #(y))\n\
                  LET z = ints.Carrier\n\
                  LET Rec = :{v :Number}\n\
                  LET w :(Rec.v) = 1";
    with_fixture(|fixture| {
        let lines = fixture.parse(source);
        fixture.in_cell(|writer, _| {
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
            let statements: Vec<usize> = (shape.path_spellings().iter())
                .map(|part| {
                    (shape.body().iter())
                        .position(|statement| source_of(statement, Site::of(part)).is_some())
                        .expect("a spelling sits in a statement")
                })
                .collect();
            assert_eq!(
                statements,
                [1, 2, 3, 4],
                "`Alias`'s, `x`'s, `f`'s and `z`'s, as met"
            );
            let callable = shape
                .nested_shapes()
                .iter()
                .find(|(_, nested)| nested.form().is_some())
                .expect("the lambda's body");
            assert!(callable.1.path_spellings().is_empty());
        })
    });
}
