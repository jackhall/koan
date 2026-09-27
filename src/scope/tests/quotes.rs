//! A quote value's code, shaped where the program loads: how each of its names resolves — a hole,
//! a `$` name where the quote is written, a `\` name where the code is built — its carried type,
//! the refusal it keeps, the names an `EVAL` offers it, and where a mark may be written.

use crate::parse::{ExpressionPart, KExpression, Mark};
use crate::scope::{
    BodyShape, Builtins, CaptureSource, Coordinate, Position, ShapeError, ShapeKind, Site, Slot,
    Target,
};
use crate::symbols::BinderSymbol;
use crate::type_lattice::KType;

use super::{Fixture, NOWHERE, builtins, located, type_name, unlocated, value_name, with_fixture};

/// Build `source` against the suites' builtins and hand the result to `check`.
fn shaped<R>(
    source: &str,
    check: impl for<'f, 'graph> FnOnce(
        &Fixture<'f, 'graph>,
        Result<&'graph BodyShape<'graph>, ShapeError<'graph>>,
    ) -> R,
) -> R {
    with_fixture(|fixture| {
        let lines = fixture.parse(source);
        fixture.in_cell(|writer| {
            let table: &Builtins = builtins(fixture, writer);
            let shape = BodyShape::of_program(
                fixture.program,
                &lines,
                table,
                fixture.types,
                fixture.scratch(),
            );
            check(fixture, shape)
        })
    })
}

/// The code shape of the quote a `LET <name> = #(…)` statement at `statement` binds.
fn code<'graph>(shape: &BodyShape<'graph>, statement: usize) -> &'graph BodyShape<'graph> {
    let quote = &shape.body()[statement].parts[3].value;
    assert!(matches!(quote, ExpressionPart::QuotedExpression(_)));
    let code = shape
        .nested(Site::of(quote))
        .expect("a quote value's code is shaped");
    assert_eq!(code.kind(), ShapeKind::Code);
    code
}

/// The nested shape at the part `index` of `node`.
fn nested<'graph>(
    shape: &BodyShape<'graph>,
    node: &KExpression<'graph>,
    index: usize,
) -> &'graph BodyShape<'graph> {
    shape
        .nested(Site::of(&node.parts[index].value))
        .expect("the part has a shape")
}

/// The node a parenthesized part holds.
fn node<'graph>(part: &ExpressionPart<'graph>) -> &'graph KExpression<'graph> {
    let ExpressionPart::Expression(node) = part else {
        panic!("a parenthesized node");
    };
    node.reference()
}

/// Each capture of `shape` as `(name, mark, source)`.
fn captures(shape: &BodyShape<'_>) -> Vec<(BinderSymbol, Option<Mark>, CaptureSource)> {
    shape
        .captures()
        .iter()
        .map(|capture| (capture.name, capture.mark, capture.source))
        .collect()
}

fn value(fixture: &Fixture<'_, '_>, text: &str) -> BinderSymbol {
    BinderSymbol::Value(value_name(text, fixture.symbols))
}

/// Read at `slot` of the reader's own activation.
fn local(slot: Slot) -> CaptureSource {
    CaptureSource::Read(Coordinate::Activation {
        hops: 0,
        target: Target::Local(slot),
    })
}

/// Read at capture `index` of the reader's own activation.
fn captured(index: u32) -> Coordinate {
    Coordinate::Activation {
        hops: 0,
        target: Target::Capture(crate::scope::CaptureSlot(index)),
    }
}

#[test]
fn a_hole_a_written_name_and_a_built_name_each_resolve_their_own_way() {
    shaped("LET x = 1\nLET q = #(PRINT x $x \\x)", |fixture, shape| {
        let shape = shape.expect("the program shapes");
        let code = code(shape, 1);
        let x = value(fixture, "x");
        let (slot, _) = shape.slot(x).unwrap();
        // Three captures of one name: the unmarked one never reaches the program's `x`.
        assert_eq!(
            captures(code),
            [
                (x, None, CaptureSource::Hole),
                (x, Some(Mark::Written), local(slot)),
                (x, Some(Mark::Built), CaptureSource::Offered),
            ]
        );
        let parts = code.body()[0].parts;
        for (index, capture) in [(1, 0), (2, 1), (3, 2)] {
            let mention = code.mention(Site::of(&parts[index].value)).unwrap();
            assert_eq!(mention.coordinate, captured(capture));
        }
    });
}

#[test]
fn a_written_name_never_binds_to_a_binder_in_its_code() {
    shaped(
        "LET x = 1\nLET q = #((LET x = 2) (PRINT $x x))",
        |fixture, shape| {
            let shape = shape.expect("the program shapes");
            let code = code(shape, 1);
            let x = value(fixture, "x");
            let (outer, _) = shape.slot(x).unwrap();
            assert_eq!(captures(code), [(x, Some(Mark::Written), local(outer))]);
            // The unmarked `x` is the code's own binder.
            let (inner, _) = code.slot(x).unwrap();
            let print = &code.body()[1];
            let mention = code.mention(Site::of(&print.parts[2].value)).unwrap();
            assert_eq!(
                mention.coordinate,
                Coordinate::Activation {
                    hops: 0,
                    target: Target::Local(inner)
                }
            );
        },
    );
}

#[test]
fn a_function_in_code_captures_each_name_through_its_mark() {
    shaped(
        "LET v = 1\nLET q = #(FN :{} -> Number = #((PRINT x) (PRINT $v) (PRINT \\y)))",
        |fixture, shape| {
            let shape = shape.expect("the program shapes");
            let code = code(shape, 1);
            let (x, v, y) = (
                value(fixture, "x"),
                value(fixture, "v"),
                value(fixture, "y"),
            );
            let (slot, _) = shape.slot(v).unwrap();
            assert_eq!(
                captures(code),
                [
                    (x, None, CaptureSource::Hole),
                    (v, Some(Mark::Written), local(slot)),
                    (y, Some(Mark::Built), CaptureSource::Offered),
                ]
            );
            let body = nested(code, &code.body()[0], 5);
            assert_eq!(body.kind(), ShapeKind::Callable);
            assert_eq!(
                captures(body),
                [
                    (x, None, CaptureSource::Read(captured(0))),
                    (v, Some(Mark::Written), CaptureSource::Read(captured(1))),
                    (y, Some(Mark::Built), CaptureSource::Read(captured(2))),
                ]
            );
        },
    );
}

#[test]
fn marks_work_on_type_names_and_a_builtin_is_no_hole() {
    shaped(
        "LET Alias = Number\nLET q = #(PRINT $Alias Carrier Number origin)",
        |fixture, shape| {
            let shape = shape.expect("the program shapes");
            let code = code(shape, 1);
            let alias = BinderSymbol::Type(type_name("Alias", fixture.symbols));
            let carrier = BinderSymbol::Type(type_name("Carrier", fixture.symbols));
            let (slot, _) = shape.slot(alias).unwrap();
            assert_eq!(
                captures(code),
                [
                    (alias, Some(Mark::Written), local(slot)),
                    (carrier, None, CaptureSource::Hole),
                ]
            );
            let parts = code.body()[0].parts;
            for index in [3, 4] {
                let mention = code.mention(Site::of(&parts[index].value)).unwrap();
                assert!(matches!(mention.coordinate, Coordinate::Builtin(_)));
            }
        },
    );
    // A marked type name in a type a definition in the code writes is the code's too.
    for source in [
        "LET Alias = Number\nLET q = #(FN :{v :($Alias)} -> Number = #(v))",
        "LET Alias = Number\nLET q = #(NEWTYPE Boxed = :(LIST OF $Alias))",
    ] {
        shaped(source, |fixture, shape| {
            let shape = shape.expect("the program shapes");
            let code = code(shape, 1);
            let alias = BinderSymbol::Type(type_name("Alias", fixture.symbols));
            let (slot, _) = shape.slot(alias).unwrap();
            assert_eq!(
                captures(code),
                [(alias, Some(Mark::Written), local(slot))],
                "`{source}`"
            );
        });
    }
}

#[test]
fn a_nested_quotes_written_name_is_a_hole_of_the_code_around_it() {
    shaped("LET q = #(PRINT #(PRINT $y))", |fixture, shape| {
        let shape = shape.expect("the program shapes");
        let outer = code(shape, 0);
        let y = value(fixture, "y");
        assert_eq!(captures(outer), [(y, None, CaptureSource::Hole)]);
        let inner = nested(outer, &outer.body()[0], 1);
        assert_eq!(inner.kind(), ShapeKind::Code);
        assert_eq!(
            captures(inner),
            [(y, Some(Mark::Written), CaptureSource::Read(captured(0)))]
        );
    });
}

#[test]
fn a_quotes_type_needs_the_built_names_no_binder_in_its_code_fills() {
    shaped(
        "LET a = #((LET x = 1) (PRINT \\x))
LET b = #(FN :{x :Number} -> Number = #(\\x))
LET c = #((PRINT \\x) (LET x = 1))
LET d = #(PRINT \\x \\y $a)
LET e = #(PRINT origin)",
        |fixture, shape| {
            let shape = shape.expect("the program shapes");
            let (x, y) = (value(fixture, "x"), value(fixture, "y"));
            assert_eq!(code(shape, 0).code_type(), KType::BLOCK);
            // A function's parameter fills its body's `\x`, so the quote needs nothing.
            let lambda = code(shape, 1);
            assert!(lambda.code_type().code_parent().is_some());
            // A `\x` read before the binder that would fill it needs `x`.
            let needing = |kind, names: &[BinderSymbol]| {
                fixture.types.code_needing(fixture.scratch(), kind, names)
            };
            assert_eq!(code(shape, 2).code_type(), needing(KType::BLOCK, &[x]));
            assert_eq!(
                code(shape, 3).code_type(),
                needing(KType::EXPRESSION, &[x, y])
            );
            assert_eq!(code(shape, 4).code_type(), KType::EXPRESSION);
        },
    );
}

#[test]
fn a_malformed_quote_loads_and_keeps_its_refusal() {
    shaped(
        "LET v = 1\nLET q = #((LET x = 1) (LET x = 2) (PRINT $v \\w))",
        |fixture, shape| {
            let shape = shape.expect("a quote's code is never refused where the program loads");
            let code = code(shape, 1);
            let x = value(fixture, "x");
            assert!(matches!(
                code.refusal().copied().map(unlocated),
                Some(ShapeError::Rebind { name, .. }) if name == x
            ));
            assert_eq!(code.statements(), 0);
            // Its `$` names are still bound where it is written.
            let (v, _) = shape.slot(value(fixture, "v")).unwrap();
            assert_eq!(
                captures(code),
                [(value(fixture, "v"), Some(Mark::Written), local(v))]
            );
            let needing =
                fixture
                    .types
                    .code_needing(fixture.scratch(), KType::BLOCK, &[value(fixture, "w")]);
            assert_eq!(code.code_type(), needing);
        },
    );
}

#[test]
fn a_written_name_nothing_binds_where_the_quote_is_written_refuses_the_program() {
    for source in [
        "LET q = #(PRINT $nope)",
        "LET q = #((LET x = 1) (LET x = 2) (PRINT $nope))",
        // `x` is the code's own, never where the quote is written.
        "LET q = #((LET x = 1) (PRINT $x))",
    ] {
        shaped(source, |_, shape| {
            let error = shape
                .err()
                .unwrap_or_else(|| panic!("`{source}` is refused"));
            assert!(
                matches!(error, ShapeError::Unbound { .. }),
                "`{source}`: {error:?}"
            );
            assert!(located(error.at()).starts_with('$'), "`{source}`");
        });
    }
}

#[test]
fn a_mark_no_quote_value_holds_is_refused() {
    for (source, at) in [
        ("LET x = 1\nPRINT $x", "$x"),
        ("LET x = 1\nPRINT \\x", "\\x"),
        ("$(PRINT origin)", "$(PRINT origin)"),
        // A body written in place is syntax of the code around it, which here is no quote value.
        ("LET v = 1\nLET f = (FN :{} -> Number = #($v))", "$v"),
        // So is a type written in a definition.
        (
            "LET Alias = Number\nNEWTYPE Boxed = :(LIST OF $Alias)",
            "$Alias",
        ),
        (
            "LET Alias = Number\nLET f = (FN :{v :($Alias)} -> Number = #(v))",
            "$Alias",
        ),
    ] {
        shaped(source, |_, shape| {
            let error = shape
                .err()
                .unwrap_or_else(|| panic!("`{source}` is refused"));
            assert_eq!(
                unlocated(error),
                ShapeError::MarkOutsideQuote { at: NOWHERE },
                "`{source}`"
            );
            assert_eq!(located(error.at()), at, "`{source}`");
        });
    }
    // The same body inside a quote value is that quote's.
    shaped(
        "LET v = 1\nLET q = #(FN :{} -> Number = #($v))",
        |_, shape| assert!(shape.is_ok()),
    );
}

#[test]
fn a_quote_reading_its_own_binder_is_a_one_member_knot() {
    shaped("LET echo = #(PRINT $echo)", |fixture, shape| {
        let shape = shape.expect("the program shapes");
        let (slot, _) = shape.slot(value(fixture, "echo")).unwrap();
        let component = shape.component_of(slot);
        assert!(component.cyclic && component.deferred_only);
        assert_eq!(component.members, [slot]);
        let code = code(shape, 0);
        assert!(matches!(
            code.captures()[0].source,
            CaptureSource::Member { index: 0, .. }
        ));
    });
}

#[test]
fn an_eval_of_a_parameter_needing_names_offers_them_where_it_is_written() {
    shaped(
        "LET twice = (FN :{body :(Expression NEEDING #[it])} -> Any = #((LET it = 5) (EVAL body)))",
        |fixture, shape| {
            let shape = shape.expect("the program shapes");
            let lambda = node(&shape.body()[0].parts[3].value);
            let body = nested(shape, lambda, 5);
            let it = value(fixture, "it");
            let (slot, _) = body.slot(it).unwrap();
            let eval = &body.body()[1];
            let offered = body.offers(Site::of(&eval.parts[1].value));
            assert_eq!(
                offered,
                [(
                    it,
                    Coordinate::Activation {
                        hops: 0,
                        target: Target::Local(slot)
                    }
                )]
            );
            // The `EVAL` reads `it` as an eager read at its statement would.
            assert_eq!(
                body.slot(it).map(|(_, declared)| declared),
                Some(Position::statement(0))
            );
        },
    );
    shaped(
        "LET twice = (FN :{body :(Expression NEEDING #[it])} -> Any = #(EVAL body))",
        |fixture, shape| {
            let error = shape.err().expect("`it` is visible nowhere at the `EVAL`");
            assert!(matches!(
                error,
                ShapeError::Unbound { name, .. } if name == value(fixture, "it")
            ));
        },
    );
    // A parameter needing nothing offers nothing.
    shaped(
        "LET twice = (FN :{body :Expression} -> Any = #(EVAL body))",
        |_, shape| {
            let shape = shape.expect("the program shapes");
            let lambda = node(&shape.body()[0].parts[3].value);
            let body = nested(shape, lambda, 5);
            let eval = &body.body()[0];
            assert!(body.offers(Site::of(&eval.parts[1].value)).is_empty());
        },
    );
}
