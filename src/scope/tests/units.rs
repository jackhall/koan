//! The order a body's units run in: each after every unit it reads, independent ones as written,
//! an `EVAL` after every binder declared before it — a body where one of those waits on the `EVAL`
//! is refused — and the last statement's unit marked.

use crate::parse::KExpression;
use crate::scope::{BodyShape, Builtins, Position, ShapeError, ShapeKind, Unit, UnitWork};

use crate::symbols::BinderSymbol;

use super::{Fixture, builtins, value_name, with_fixture};

/// Shape `source` and hand `check` the program shape.
fn shaped<R>(
    source: &str,
    check: impl for<'f, 'graph> FnOnce(&Fixture<'f, 'graph>, &'graph BodyShape<'graph>) -> R,
) -> R {
    with_fixture(|fixture| {
        let lines: Vec<KExpression<'_>> = fixture.parse(source);
        fixture.in_cell(|writer| {
            let table: &Builtins = builtins(fixture, writer);
            let shape = BodyShape::of_program(fixture.program, &lines, table, fixture.scratch())
                .unwrap_or_else(|error| panic!("`{source}` shapes: {error:?}"));
            check(fixture, shape)
        })
    })
}

/// The statement each unit is keyed by — a component's lowest, or the statement itself — in the
/// order the units run.
fn order(shape: &BodyShape<'_>) -> Vec<u32> {
    shape
        .units()
        .iter()
        .map(|unit| match unit.work {
            UnitWork::Statement(statement) => statement,
            UnitWork::Component(component) => shape.components()[component.index()]
                .members
                .iter()
                .map(|slot| {
                    let (_, position) = shape.slot(shape.slot_name(*slot)).unwrap();
                    position.0 - 1
                })
                .min()
                .unwrap(),
        })
        .collect()
}

/// The statement the unit marked `last` is keyed by.
fn last(shape: &BodyShape<'_>) -> Option<u32> {
    let units: Vec<Unit> = shape.units().to_vec();
    let marked: Vec<usize> = (0..units.len()).filter(|at| units[*at].last).collect();
    assert!(marked.len() <= 1, "one unit holds the last statement");
    marked.first().map(|at| order(shape)[*at])
}

#[test]
fn independent_statements_run_as_written() {
    shaped("LET a = 1\nLET b = 2\n(a)\nLET c = 3", |_, shape| {
        assert_eq!(order(shape), [0, 1, 2, 3]);
        assert_eq!(last(shape), Some(3));
    });
}

#[test]
fn a_forward_capture_runs_its_binder_first_and_last_marks_the_last_statement() {
    shaped("LET f = (FN :{} -> Number = (g))\nLET g = 5", |_, shape| {
        assert_eq!(order(shape), [1, 0]);
        assert_eq!(last(shape), Some(1), "the last statement's unit runs first");
    });
}

#[test]
fn a_statement_binding_nothing_follows_what_it_reads() {
    shaped(
        "LET y = 2\n(FN :{} -> Number = (x))\nLET x = 1",
        |_, shape| {
            assert_eq!(order(shape), [0, 2, 1]);
        },
    );
}

#[test]
fn a_component_is_one_unit() {
    shaped(
        "LET even = (FN :{} -> Number = (odd))\nLET odd = (FN :{} -> Number = (even))\n(odd)",
        |_, shape| {
            assert_eq!(shape.units().len(), 2);
            assert_eq!(order(shape), [0, 2]);
        },
    );
}

#[test]
fn an_eval_follows_every_binder_declared_before_it_and_no_later_one() {
    shaped(
        "LET f = (FN :{} -> Number = (g))\n(EVAL #(origin))\nLET g = 5",
        |_, shape| {
            // Without the rule the `EVAL`, independent of both, would run before `f`'s unit.
            assert_eq!(order(shape), [2, 0, 1]);
        },
    );
    shaped("(EVAL #(origin))\nLET h = 9", |_, shape| {
        assert_eq!(order(shape), [0, 1]);
    });
}

#[test]
fn an_eval_a_binder_before_it_waits_on_is_refused() {
    // `f` is visible to the `EVAL` and reads `g`, which the `EVAL` binds: neither can run first.
    // Declaring `f` after `g` would hide it from the `EVAL` and shape.
    with_fixture(|fixture| {
        for source in [
            "LET f = (FN :{} -> Number = (g))\nLET g = (EVAL #(origin))",
            // Through a binder between them.
            "LET f = (FN :{} -> Number = (h))\nLET h = (FN :{} -> Number = (g))\nLET g = (EVAL #(origin))",
        ] {
            let lines: Vec<KExpression<'_>> = fixture.parse(source);
            fixture.in_cell(|writer| {
                let table: &Builtins = builtins(fixture, writer);
                let shape =
                    BodyShape::of_program(fixture.program, &lines, table, fixture.scratch());
                let Err(ShapeError::EvalCycle { name, eval }) = shape else {
                    panic!(
                        "`{source}` is refused as an `EVAL` cycle: {:?}",
                        shape.map(|_| ())
                    );
                };
                assert_eq!(eval, Position::statement(lines.len() - 1));
                let named =
                    ["f", "h"].map(|text| BinderSymbol::Value(value_name(text, fixture.symbols)));
                assert!(
                    named.contains(&name),
                    "`{source}` names a binder on the cycle"
                );
            });
        }
    });
    shaped(
        "LET g = (EVAL #(origin))\nLET f = (FN :{} -> Number = (g))",
        |_, shape| assert_eq!(order(shape), [0, 1]),
    );
}

#[test]
fn a_parameter_is_no_unit() {
    shaped("LET f = (FN :{x :Number} -> Number = (x))", |_, shape| {
        let (_, body) = shape
            .nested_shapes()
            .iter()
            .find(|(_, nested)| nested.kind() == ShapeKind::Callable)
            .expect("`f` births a body");
        assert_eq!(order(body), [0]);
        assert_eq!(last(body), Some(0));
    });
}
