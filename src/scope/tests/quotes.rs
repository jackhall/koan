//! A quote value's code, shaped where the program loads: how each of its names resolves — a hole,
//! a `$` name where the quote is written, a `\` name where the code is built — its carried type,
//! the refusal it keeps, the names an `EVAL` offers it, and where a mark may be written.

use crate::parse::{ExpressionPart, KExpression, Mark};
use crate::scope::{
    BodyShape, Builtins, CaptureSource, Coordinate, Offer, Position, ShapeError, ShapeKind, Slot,
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
        fixture.in_cell(|writer, _| {
            let table: &Builtins = builtins(fixture, writer);
            let shape = crate::scope::Shapes::of_program(
                fixture.program,
                &lines,
                table,
                fixture.types,
                fixture.symbols,
                fixture.scratch(),
            )
            .map(|shapes| shapes.root());
            check(fixture, shape)
        })
    })
}

/// The code shape of the quote a `LET <name> = #(…)` statement at `statement` binds.
fn code<'graph>(shape: &BodyShape<'graph>, statement: usize) -> &'graph BodyShape<'graph> {
    let quote = &shape.body()[statement].parts[3].value;
    assert!(matches!(quote, ExpressionPart::QuotedExpression(_)));
    let code = shape
        .nested(shape.site(quote))
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
        .nested(shape.site(&node.parts[index].value))
        .expect("the part has a shape")
}

/// The node a parenthesized part holds.
fn node<'graph>(part: &ExpressionPart<'graph>) -> &'graph KExpression<'graph> {
    let ExpressionPart::Expression(node) = part else {
        panic!("a parenthesized node");
    };
    node
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

/// The keyworded hole an unmarked use at the key `text` spells leaves open in its code.
fn hole(fixture: &Fixture<'_, '_>, text: &str) -> (BinderSymbol, Option<Mark>, CaptureSource) {
    let key = fixture.symbols.key(text).expect("a key");
    (BinderSymbol::Key(key), None, CaptureSource::Hole)
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
        // Three captures of one name: the unmarked one never reaches the program's `x`. The use
        // itself is a keyworded hole, resolved before its arguments.
        assert_eq!(
            captures(code),
            [
                hole(fixture, "PRINT _ _ _"),
                (x, None, CaptureSource::Hole),
                (x, Some(Mark::DOLLAR), local(slot)),
                (x, Some(Mark::Built), CaptureSource::Offered),
            ]
        );
        let parts = code.body()[0].parts;
        for (index, capture) in [(1, 1), (2, 2), (3, 3)] {
            let mention = code.mention(code.site(&parts[index].value)).unwrap();
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
            assert_eq!(
                captures(code),
                [
                    hole(fixture, "PRINT _ _"),
                    (x, Some(Mark::DOLLAR), local(outer))
                ]
            );
            // The unmarked `x` is the code's own binder.
            let (inner, _) = code.slot(x).unwrap();
            let print = &code.body()[1];
            let mention = code.mention(code.site(&print.parts[2].value)).unwrap();
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
            let print = hole(fixture, "PRINT _");
            assert_eq!(
                captures(code),
                [
                    print,
                    (x, None, CaptureSource::Hole),
                    (v, Some(Mark::DOLLAR), local(slot)),
                    (y, Some(Mark::Built), CaptureSource::Offered),
                ]
            );
            let body = nested(code, &code.body()[0], 5);
            assert_eq!(body.kind(), ShapeKind::Callable);
            // The three uses share one hole.
            assert_eq!(
                captures(body),
                [
                    (print.0, None, CaptureSource::Read(captured(0))),
                    (x, None, CaptureSource::Read(captured(1))),
                    (v, Some(Mark::DOLLAR), CaptureSource::Read(captured(2))),
                    (y, Some(Mark::Built), CaptureSource::Read(captured(3))),
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
                    hole(fixture, "PRINT _ _ _ _"),
                    (alias, Some(Mark::DOLLAR), local(slot)),
                    (carrier, None, CaptureSource::Hole),
                ]
            );
            let parts = code.body()[0].parts;
            for index in [3, 4] {
                let mention = code.mention(code.site(&parts[index].value)).unwrap();
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
                [(alias, Some(Mark::DOLLAR), local(slot))],
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
        let print = hole(fixture, "PRINT _");
        assert_eq!(captures(outer), [print, (y, None, CaptureSource::Hole)]);
        let inner = nested(outer, &outer.body()[0], 1);
        assert_eq!(inner.kind(), ShapeKind::Code);
        assert_eq!(
            captures(inner),
            [
                print,
                (y, Some(Mark::DOLLAR), CaptureSource::Read(captured(1)))
            ]
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
                [(value(fixture, "v"), Some(Mark::DOLLAR), local(v))]
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
            code.captures()[1].source,
            CaptureSource::Member { index: 0, .. }
        ));
    });
}

#[test]
fn an_eval_of_a_parameter_needing_names_offers_them_where_it_is_written() {
    shaped(
        "LET twice = (FN :{body :(Expression NEEDING #[it])} -> Any = #((LET it = 5) (EVAL body -> Any)))",
        |fixture, shape| {
            let shape = shape.expect("the program shapes");
            let lambda = node(&shape.body()[0].parts[3].value);
            let body = nested(shape, lambda, 5);
            let it = value(fixture, "it");
            let (slot, _) = body.slot(it).unwrap();
            let eval = &body.body()[1];
            let offered = body.offers(body.site(&eval.parts[1].value));
            let [(name, Offer::Name(at))] = offered else {
                panic!("one name is offered");
            };
            assert_eq!(*name, it);
            assert_eq!(
                *at,
                Coordinate::Activation {
                    hops: 0,
                    target: Target::Local(slot)
                }
            );
            // The `EVAL` reads `it` as an eager read at its statement would.
            assert_eq!(
                body.slot(it).map(|(_, declared)| declared),
                Some(Position::statement(0))
            );
        },
    );
    shaped(
        "LET twice = (FN :{body :(Expression NEEDING #[it])} -> Any = #(EVAL body -> Any))",
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
        "LET twice = (FN :{body :Expression} -> Any = #(EVAL body -> Any))",
        |_, shape| {
            let shape = shape.expect("the program shapes");
            let lambda = node(&shape.body()[0].parts[3].value);
            let body = nested(shape, lambda, 5);
            let eval = &body.body()[0];
            assert!(body.offers(body.site(&eval.parts[1].value)).is_empty());
        },
    );
}

#[test]
fn a_refused_code_hands_nothing_to_the_next_body() {
    // The code surfaces `Shown`'s head into the `USING` block, then refuses its operator run; the
    // next body drafted must not take that head as a registration of its own.
    shaped(
        "LET q = #((SIG Shown = #[(EXPR #(SHOW _ :Number) -> Str)]) (USING (m :! Shown) SCOPE (1 + 2 == 3)))\nLET f = (FN :{} -> Number = #(1))",
        |fixture, shape| {
            let shape = shape.expect("a quote's code is never refused where the program loads");
            assert!(matches!(
                code(shape, 0).refusal(),
                Some(ShapeError::MixedGroups { .. })
            ));
            let (f, _) = shape.slot(value(fixture, "f")).unwrap();
            let body = shape.births(f).expect("`f` births its body");
            assert!(body.registrations().is_empty());
            assert_eq!(body.slots(), 0);
        },
    );
}

/// The shapes of the quote a `LET <name> = #(…)` statement at `statement` of `source` binds, built
/// alone under the program's claims, as code composed at run time is.
fn alone<R>(
    source: &str,
    statement: usize,
    check: impl for<'f, 'graph> FnOnce(&Fixture<'f, 'graph>, &'graph crate::scope::Shapes<'graph>) -> R,
) -> R {
    with_fixture(|fixture| {
        let lines = fixture.parse(source);
        fixture.in_cell(|writer, _| {
            let table: &Builtins = builtins(fixture, writer);
            let program = crate::scope::Shapes::of_program(
                fixture.program,
                &lines,
                table,
                fixture.types,
                fixture.symbols,
                fixture.scratch(),
            )
            .unwrap_or_else(|_| panic!("`{source}` shapes"));
            let ExpressionPart::QuotedExpression(syntax) = lines[statement].parts[3].value else {
                panic!("a quote");
            };
            let claims = program.root().group_frame().claims();
            let shapes = crate::scope::Shapes::of_code(
                fixture.program,
                (syntax, &[]),
                claims,
                table,
                fixture.types,
                fixture.symbols,
                fixture.scratch(),
            );
            check(fixture, shapes)
        })
    })
}

#[test]
fn a_code_built_alone_numbers_its_segments_from_zero() {
    let source =
        "LET q = #((LET f = (FN :{} -> Number = #(1))) (LET g = (FN :{} -> Number = #(2))))";
    alone(source, 0, |_, shapes| {
        let root = shapes.root();
        assert_eq!(root.kind(), ShapeKind::Code);
        assert!(root.refusal().is_none());
        // Two callables, then the root last.
        assert_eq!(root.segment().0, 2);
        for segment in 0..2 {
            let shape = shapes.shape_at(crate::scope::Segment(segment));
            assert_eq!(shape.segment().0, segment);
            assert_eq!(shape.kind(), ShapeKind::Callable);
        }
    });
    // Code that cannot be built keeps its refusal, and needs every `\` name it writes.
    alone(
        "LET q = #((LET x = 1) (LET x = 2) (PRINT \\w))",
        0,
        |fixture, shapes| {
            let root = shapes.root();
            assert!(root.refusal().is_some());
            assert_eq!(root.segment().0, 0);
            let w = value(fixture, "w");
            assert_eq!(
                root.code_type(),
                fixture
                    .types
                    .code_needing(fixture.scratch(), KType::BLOCK, &[w])
            );
        },
    );
}

#[test]
fn a_root_dollar_name_is_an_entry_capture() {
    alone(
        "LET x = 1\nLET q = #(PRINT x $x \\x)",
        1,
        |fixture, shapes| {
            let code = shapes.root();
            let x = value(fixture, "x");
            // Where the written quote reads the program's `x`, code built alone binds it at entry.
            assert_eq!(
                captures(code),
                [
                    hole(fixture, "PRINT _ _ _"),
                    (x, None, CaptureSource::Hole),
                    (x, Some(Mark::DOLLAR), CaptureSource::Entry { head: false }),
                    (x, Some(Mark::Built), CaptureSource::Offered),
                ]
            );
        },
    );
    // A `$` name read only at a call's head may be bound to a quantified function.
    let source = "LET f = (FN :{n :Number} -> Number = #(n))\nLET q = #($f 1)";
    alone(source, 1, |fixture, shapes| {
        let f = value(fixture, "f");
        assert_eq!(
            captures(shapes.root()),
            [(f, Some(Mark::DOLLAR), CaptureSource::Entry { head: true })]
        );
    });
}

#[test]
fn a_root_dollar_spread_is_a_spread_capture() {
    let source = "EXPR #(GREET x :Any) -> Any = #(x)\nLET q = #(PRINT $(GREET 1) $(PRINT 2))";
    alone(source, 1, |fixture, shapes| {
        let code = shapes.root();
        let parts = code.body()[0].parts;
        let ExpressionPart::MarkedUse(_, greet) = parts[1].value else {
            panic!("a marked use");
        };
        let list = code.candidates(code.site_of_node(greet)).unwrap();
        let greet_key = BinderSymbol::Key(fixture.symbols.key("GREET _").unwrap());
        let at = code
            .captures()
            .iter()
            .position(|capture| capture.name == greet_key)
            .expect("the key is captured") as u32;
        assert_eq!(
            code.captures()[at as usize].source,
            CaptureSource::Entry { head: true }
        );
        assert_eq!(
            list.candidates,
            [crate::scope::Candidate::Spread(captured(at))]
        );
        assert_eq!(list.classes, [0]);
        // A builtin key lists its builtins beside its entry list.
        let ExpressionPart::MarkedUse(_, print) = parts[2].value else {
            panic!("a marked use");
        };
        let list = code.candidates(code.site_of_node(print)).unwrap();
        assert!(matches!(
            list.candidates.last(),
            Some(crate::scope::Candidate::Spread(_))
        ));
        assert!(list.candidates.len() > 1);
    });
}

/// How a test remarks parsed syntax: the `n`-th `$` name (from 0) marked anew, or each `$(…)` use
/// made a use filled through a scope at its first key.
#[derive(Clone, Copy)]
enum Remark {
    NthDollar(usize, Mark),
    FillFirstKey(crate::parse::Scope),
}

/// `node` copied through `fixture`'s program storage, each part `remark` touches replaced, nested
/// nodes first: hand-built composed syntax over a parsed quote. `seen` counts `$` names met.
fn remarked<'graph>(
    fixture: &Fixture<'_, 'graph>,
    node: &KExpression<'graph>,
    remark: Remark,
    seen: &mut usize,
) -> &'graph KExpression<'graph> {
    let writer = fixture.program.writer();
    let mut parts = Vec::new();
    for part in node.parts {
        let value = match (part.value, remark) {
            (ExpressionPart::Expression(inner), _) => {
                ExpressionPart::Expression(remarked(fixture, inner, remark, seen))
            }
            (ExpressionPart::QuotedExpression(inner), _) => {
                ExpressionPart::QuotedExpression(remarked(fixture, inner, remark, seen))
            }
            (ExpressionPart::MarkedUse(Mark::DOLLAR, inner), Remark::FillFirstKey(scope)) => {
                let inner = remarked(fixture, inner, remark, seen);
                let key = crate::parse::KeyElement::key(inner.stored_key()[..3].iter().copied());
                let keys = crate::memory::collect(writer, std::iter::once(key));
                ExpressionPart::FilledUse(scope, keys, inner)
            }
            (ExpressionPart::MarkedUse(mark, inner), _) => {
                ExpressionPart::MarkedUse(mark, remarked(fixture, inner, remark, seen))
            }
            (ExpressionPart::MarkedName(Mark::DOLLAR, name), Remark::NthDollar(n, mark)) => {
                *seen += 1;
                let mark = if *seen == n + 1 { mark } else { Mark::DOLLAR };
                ExpressionPart::MarkedName(mark, name)
            }
            (other, _) => other,
        };
        parts.push(crate::source::Spanned {
            value,
            span: part.span,
        });
    }
    KExpression::nested(writer, &parts, node.source)
}

/// [`alone`] over the quote's syntax [`remarked`] by `remark`.
fn alone_remarked<R>(
    source: &str,
    statement: usize,
    remark: Remark,
    check: impl for<'f, 'graph> FnOnce(&Fixture<'f, 'graph>, &'graph crate::scope::Shapes<'graph>) -> R,
) -> R {
    with_fixture(|fixture| {
        let lines = fixture.parse(source);
        fixture.in_cell(|writer, _| {
            let table: &Builtins = builtins(fixture, writer);
            let program = crate::scope::Shapes::of_program(
                fixture.program,
                &lines,
                table,
                fixture.types,
                fixture.symbols,
                fixture.scratch(),
            )
            .unwrap_or_else(|_| panic!("`{source}` shapes"));
            let ExpressionPart::QuotedExpression(syntax) = lines[statement].parts[3].value else {
                panic!("a quote");
            };
            let syntax = remarked(fixture, syntax, remark, &mut 0);
            let claims = program.root().group_frame().claims();
            let shapes = crate::scope::Shapes::of_code(
                fixture.program,
                (syntax, &[]),
                claims,
                table,
                fixture.types,
                fixture.symbols,
                fixture.scratch(),
            );
            check(fixture, shapes)
        })
    })
}

/// A scope numbered `number`, pinned or not.
fn scope(number: u32, pinned: bool) -> crate::parse::Scope {
    crate::parse::Scope { number, pinned }
}
#[test]
fn two_scopes_of_one_dollar_name_are_two_entry_captures() {
    let source = "LET x = 1\nLET q = #(PRINT $x $x)";
    alone_remarked(
        source,
        1,
        Remark::NthDollar(1, Mark::Written(scope(1, false))),
        |fixture, shapes| {
            let x = value(fixture, "x");
            assert_eq!(
                captures(shapes.root()),
                [
                    hole(fixture, "PRINT _ _"),
                    (x, Some(Mark::DOLLAR), CaptureSource::Entry { head: false }),
                    (
                        x,
                        Some(Mark::Written(scope(1, false))),
                        CaptureSource::Entry { head: false }
                    ),
                ]
            );
        },
    );
}

#[test]
fn a_filled_name_skips_a_binder_of_its_name() {
    let source = "LET x = 1\nLET q = #((LET x = 2) (PRINT $x))";
    let filled = Mark::Filled(scope(1, false));
    alone_remarked(
        source,
        1,
        Remark::NthDollar(0, filled),
        |fixture, shapes| {
            let x = value(fixture, "x");
            let code = captures(shapes.root());
            assert!(
                code.contains(&(x, Some(filled), CaptureSource::Entry { head: false })),
                "{code:?}"
            );
        },
    );
}

#[test]
fn a_filled_use_fills_only_the_keys_it_lists() {
    let source = "LET q = #(PRINT $(1 + 2 - 3))";
    let filled = scope(1, false);
    alone_remarked(
        source,
        0,
        Remark::FillFirstKey(filled),
        |fixture, shapes| {
            let plus = BinderSymbol::Key(fixture.symbols.key("_ + _").unwrap());
            let minus = BinderSymbol::Key(fixture.symbols.key("_ - _").unwrap());
            let code = captures(shapes.root());
            assert!(
                code.contains(&(
                    plus,
                    Some(Mark::Filled(filled)),
                    CaptureSource::Entry { head: true }
                )),
                "{code:?}"
            );
            assert!(
                code.contains(&(minus, None, CaptureSource::Hole)),
                "{code:?}"
            );
            assert!(
                !code
                    .iter()
                    .any(|(name, mark, _)| *name == plus && mark.is_none())
            );
        },
    );
}

#[test]
fn a_pinned_mark_in_a_nested_quote_is_the_roots_entry_capture() {
    // The second `$x` sits in the nested quote, pinned to the root's binding; the third is that
    // quote's own, an ordinary name of the code around it.
    let source = "LET x = 1\nLET q = #(PRINT $x #(PRINT $x $x))";
    alone_remarked(
        source,
        1,
        Remark::NthDollar(1, Mark::Written(scope(0, true))),
        |fixture, shapes| {
            let x = value(fixture, "x");
            let root = shapes.root();
            let code = captures(root);
            let entries = (code.iter())
                .filter(|(name, _, source)| {
                    *name == x && matches!(source, CaptureSource::Entry { .. })
                })
                .count();
            assert_eq!(entries, 1, "one shared entry capture: {code:?}");
            assert!(code.contains(&(x, None, CaptureSource::Hole)), "{code:?}");
            let inner = &root.body()[0].parts[2].value;
            let nested = root
                .nested(root.site(inner))
                .expect("the nested quote is shaped");
            let pinned = (nested.captures().iter())
                .find(|capture| capture.mark == Some(Mark::Written(scope(0, true))))
                .expect("the nested quote captures its pinned mark");
            assert!(matches!(pinned.source, CaptureSource::Read(_)));
            assert!(
                nested
                    .captures()
                    .iter()
                    .any(|capture| capture.mark == Some(Mark::DOLLAR))
            );
        },
    );
}
