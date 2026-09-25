//! Code read where it is written: the static check every builtin part a role reads as written
//! passes, a callable's quoted body and head, an arm set as a dict of quotes, a union's variants, a
//! `FOR ALL` group, a signature's member list, a field label, and a value dict's `_` key.

use crate::parse::builtin_shapes::BuiltinShapeId;
use crate::parse::builtin_shapes::role::Heads;
use crate::parse::{ExpressionPart, KExpression};
use crate::scope::{BodyShape, Builtins, Position, QuotedPart, ShapeError, ShapeKind, Site};
use crate::symbols::BinderSymbol;
use crate::type_lattice::KType;

use super::{Fixture, NOWHERE, builtins, located, type_name, unlocated, value_name, with_fixture};

/// Build `source` against the suites' builtins and hand the result to `check`.
fn shaped<R>(
    source: &str,
    check: impl for<'f, 'graph> FnOnce(
        &Fixture<'f, 'graph>,
        &[KExpression<'graph>],
        Result<&'graph BodyShape<'graph>, ShapeError>,
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
            check(fixture, &lines, shape)
        })
    })
}

/// The error building `source` refuses with.
fn refusal(source: &str) -> ShapeError {
    shaped(source, |_, _, shape| match shape {
        Ok(_) => panic!("`{source}` is refused"),
        Err(error) => error,
    })
}

/// The node a parenthesized part holds.
fn node<'graph>(part: &ExpressionPart<'graph>) -> &'graph KExpression<'graph> {
    let ExpressionPart::Expression(node) = part else {
        panic!("a parenthesized node");
    };
    node.reference()
}

/// The shape of the `FN` a `LET f = (FN …)` line births.
fn lambda_body<'graph>(
    shape: &BodyShape<'graph>,
    line: &KExpression<'graph>,
) -> &'graph BodyShape<'graph> {
    let lambda = node(&line.parts[3].value);
    shape
        .nested(Site::of_body(lambda).expect("a lambda has a body"))
        .expect("the body has a shape")
}

/// Each arm of the `MATCH` or `TRY` at `statement`'s root, beside its guard.
fn arms<'graph>(
    shape: &BodyShape<'graph>,
    statement: &KExpression<'graph>,
    at: usize,
) -> Vec<(&'graph ExpressionPart<'graph>, &'graph BodyShape<'graph>)> {
    let ExpressionPart::DictLiteral(pairs) = statement.statement_spine().parts[at].value else {
        panic!("the arms are a dict");
    };
    pairs
        .iter()
        .map(|(guard, arm)| {
            let arm_shape = shape.nested(Site::of(arm)).expect("an arm has a shape");
            (guard, arm_shape)
        })
        .collect()
}

#[test]
fn a_quoted_body_is_its_callables_body_shape() {
    shaped(
        "LET f = (FN :{x :Number} -> Number = #(x))",
        |fixture, lines, shape| {
            let shape = shape.expect("the program shapes");
            let body = lambda_body(shape, &lines[0]);
            assert_eq!(body.kind(), ShapeKind::Callable);
            assert!(
                body.slot(BinderSymbol::Value(value_name("x", fixture.symbols)))
                    .is_some()
            );
            let form = body.form().expect("a callable knows its form");
            assert_eq!(
                form.parts.as_ptr(),
                node(&lines[0].parts[3].value).parts.as_ptr()
            );
        },
    );
}

#[test]
fn a_part_not_written_as_its_role_reads_it_is_refused() {
    let at = NOWHERE;
    let unquoted = |form, part| ShapeError::Unquoted { form, part, at };
    let cases = [
        (
            "LET f = (FN :{} -> Number = (1))",
            unquoted(BuiltinShapeId::Lambda, QuotedPart::Body),
            "(1)",
        ),
        (
            "LET f = (FN :{} -> Number = x)",
            unquoted(BuiltinShapeId::Lambda, QuotedPart::Body),
            "x",
        ),
        (
            "EXPR (FOO a :Number) -> Number = #(a)",
            unquoted(BuiltinShapeId::ExpressionDefinition, QuotedPart::Head),
            "(FOO a :Number)",
        ),
        (
            "MATCH 1 -> :Number WITH (Number -> (1))",
            unquoted(BuiltinShapeId::Match, QuotedPart::Arms),
            "(Number -> (1))",
        ),
        (
            "MATCH 1 -> :Number WITH {Number: (1)}",
            unquoted(BuiltinShapeId::Match, QuotedPart::Arms),
            "{Number: (1)}",
        ),
        (
            "MATCH 1 -> :Number WITH #(x)",
            unquoted(BuiltinShapeId::Match, QuotedPart::Arms),
            "#(x)",
        ),
        (
            "UNION Mb = (Some :Number)",
            unquoted(BuiltinShapeId::Union, QuotedPart::Variants),
            "(Some :Number)",
        ),
        (
            "LET f = (FN FOR ALL (Elt) :{x :Elt} -> Elt = #(x))",
            unquoted(BuiltinShapeId::QuantifiedLambda, QuotedPart::Quantifiers),
            "(Elt)",
        ),
        (
            "SIG Sg = (VAL x :Str)",
            unquoted(BuiltinShapeId::Sig, QuotedPart::Members),
            "(VAL x :Str)",
        ),
        (
            "MODULE m = #(LET x = 1)",
            ShapeError::Malformed {
                form: BuiltinShapeId::Module,
                at,
            },
            "#(LET x = 1)",
        ),
        (
            "LET #(x) = 1",
            ShapeError::Malformed {
                form: BuiltinShapeId::LetValue,
                at,
            },
            "#(x)",
        ),
        (
            "LET f = (FN :{} -> Number = 42)",
            ShapeError::Inadmissible {
                form: BuiltinShapeId::Lambda,
                index: 5,
                slot: KType::BLOCK,
                at,
            },
            "42",
        ),
        (
            "MODULE m = 42",
            ShapeError::Inadmissible {
                form: BuiltinShapeId::Module,
                index: 3,
                slot: KType::BLOCK,
                at,
            },
            "42",
        ),
        // A type guard is type code, and a value is none.
        (
            "MATCH 1 -> :Number WITH #{1: (a)}",
            ShapeError::Inadmissible {
                form: BuiltinShapeId::Match,
                index: 5,
                slot: KType::DICT_TYPE_CODE_BLOCK,
                at,
            },
            "#{1: (a)}",
        ),
        // A label guard is a name, and a compound type is none.
        (
            "UNION Mb = #{Some: Number}\nMATCH 1 OVER Mb -> :Number WITH #{:(Number | Str): (it)}",
            ShapeError::Inadmissible {
                form: BuiltinShapeId::MatchOver,
                index: 7,
                slot: KType::DICT_NAME_BLOCK,
                at,
            },
            "#{:(Number | Str): (it)}",
        ),
        // A type expression and a `TRY` or `CATCH` operand are bare: a quote there is refused.
        (
            "MATCH 1 -> #(Number) WITH #{Number: (it)}",
            ShapeError::Malformed {
                form: BuiltinShapeId::Match,
                at,
            },
            "#(Number)",
        ),
        (
            "TRY #(1) -> :Number WITH #{_: (0)}",
            ShapeError::Malformed {
                form: BuiltinShapeId::Try,
                at,
            },
            "#(1)",
        ),
        (
            "CATCH #(1)",
            ShapeError::Malformed {
                form: BuiltinShapeId::Catch,
                at,
            },
            "#(1)",
        ),
        (
            "SIG Sg = #[(PRINT 1)]",
            ShapeError::Inadmissible {
                form: BuiltinShapeId::Sig,
                index: 3,
                slot: KType::LIST_OF_DECLARATION,
                at,
            },
            "#[(PRINT 1)]",
        ),
        // A `FOR ALL` element that quotes no lone name, in a type expression as anywhere.
        (
            "LET Over = :(FN FOR ALL #[(Elt OVER Value)] :{x :Number} -> Number)",
            ShapeError::Inadmissible {
                form: BuiltinShapeId::QuantifiedLambdaType,
                index: 3,
                slot: KType::QUANTIFIER_CODE,
                at,
            },
            "#[(Elt OVER Value)]",
        ),
        // `{}` is the empty record, so a union with no variant cannot be written.
        (
            "UNION Empty = #{}",
            ShapeError::Inadmissible {
                form: BuiltinShapeId::Union,
                index: 3,
                slot: KType::DICT_NAME_TYPE_CODE,
                at,
            },
            "#{}",
        ),
        (
            "NEWTYPE Dist = (LIST OF Number)",
            ShapeError::Inadmissible {
                form: BuiltinShapeId::NewTypeDefinition,
                index: 3,
                slot: KType::TYPE_CODE,
                at,
            },
            "(LIST OF Number)",
        ),
    ];
    for (source, expected, text) in cases {
        let error = refusal(source);
        assert_eq!(located(error.at()), text, "{source}");
        assert_eq!(unlocated(error), expected, "{source}");
    }
    shaped(
        "MATCH 1 -> :Number WITH #{:(Number | Str): (it), :{x :Number} : (0)}",
        |_, _, shape| {
            assert!(shape.is_ok(), "a compound type is a type guard");
        },
    );
    // A manifest value member is a declaration by type, refused only where the signature is
    // elaborated.
    shaped("SIG Sg = #[(LET x = 1)]", |_, _, shape| {
        assert!(shape.is_ok(), "a binder is a declaration");
    });
}

#[test]
fn a_heads_names_and_a_groups_names_are_the_bodys() {
    shaped(
        "EXPR #(ECHO v :Number) -> Number = #(v)",
        |fixture, lines, shape| {
            let shape = shape.expect("the program shapes");
            let body = shape
                .nested(Site::of_body(&lines[0]).expect("a definition has a body"))
                .expect("the body has a shape");
            let (_, at) = body
                .slot(BinderSymbol::Value(value_name("v", fixture.symbols)))
                .expect("the head's name is a parameter");
            assert_eq!(at, Position::PARAMETER);
        },
    );
    for group in ["#[Elt]", "#{Elt: Ring}"] {
        let source = format!("LET f = (FN FOR ALL {group} :{{x :Elt}} -> Elt = #(x))");
        shaped(&source, |fixture, _, shape| {
            let shape = shape.expect("the program shapes");
            let mentioned = |name: &str| {
                let name = BinderSymbol::Type(type_name(name, fixture.symbols));
                shape.mentions().iter().any(|mention| mention.name == name)
            };
            assert!(!mentioned("Elt"), "{group}: a quantifier is no mention");
            assert_eq!(mentioned("Ring"), group.contains("Ring"), "{group}");
        });
    }
}

#[test]
fn an_arm_is_a_block_binding_it_and_knows_its_guard() {
    shaped(
        "LET q = 1\nMATCH q -> :Number WITH #{Number: (it), _: (q)}",
        |fixture, lines, shape| {
            let shape = shape.expect("the program shapes");
            let arms = arms(shape, &lines[1], 5);
            let ExpressionPart::DictLiteral(pairs) = lines[1].parts[5].value else {
                panic!("the arms are a dict");
            };
            for ((guard, arm), (written, _)) in arms.iter().zip(pairs.iter()) {
                assert_eq!(arm.kind(), ShapeKind::Block);
                assert!(
                    arm.slot(BinderSymbol::Value(value_name("it", fixture.symbols)))
                        .is_some()
                );
                let facts = arm.arm().expect("an arm knows it is one");
                assert_eq!(facts.heads, Heads::Types);
                assert!(!facts.tail, "the top level is no tail body");
                match facts.guard {
                    Some(held) => {
                        assert!(std::ptr::eq(held, *guard) && std::ptr::eq(held, written))
                    }
                    None => assert!(guard.is_wildcard(), "only `_` guards nothing"),
                }
            }
            assert!(arms[1].1.arm().expect("an arm").guard.is_none());
        },
    );
}

#[test]
fn an_arm_is_in_tail_position_exactly_where_its_match_is() {
    let tail_of = |body: &str| -> Vec<bool> {
        let source = format!("LET f = (FN :{{x :Number}} -> Number = #({body}))");
        shaped(&source, |fixture, lines, shape| {
            let shape = shape.unwrap_or_else(|error| {
                panic!(
                    "`{source}` shapes: {}",
                    error.display(fixture.symbols, fixture.types)
                )
            });
            let body = lambda_body(shape, &lines[0]);
            let mut tails = Vec::new();
            let mut pending = vec![body];
            while let Some(shape) = pending.pop() {
                for (_, nested) in shape.nested_shapes() {
                    if let Some(arm) = nested.arm() {
                        tails.push(arm.tail);
                    }
                    pending.push(nested);
                }
            }
            tails
        })
    };
    let arm = "MATCH x -> :Number WITH #{Number: (it)}";
    assert_eq!(tail_of(arm), [true], "the body's last statement");
    assert_eq!(
        tail_of("MATCH x -> :Number WITH #{Number: (MATCH it -> :Number WITH #{Number: (it)})}"),
        [true, true],
        "an arm's tail inside a tail arm"
    );
    assert_eq!(tail_of(&format!("\n  {arm}\n  x\n")), [false], "not last");
    assert_eq!(tail_of(&format!("PRINT ({arm})")), [false], "under a call");
    assert_eq!(
        tail_of(&format!("\n  LET y = ({arm})\n  y\n")),
        [false],
        "a binding's right-hand side"
    );
    assert_eq!(
        tail_of(&format!("\n  x\n  LET y = ({arm})\n")),
        [false],
        "a last statement that binds"
    );
    shaped(
        "MODULE m = (MATCH 1 -> :Number WITH #{Number: (it)})",
        |_, _, shape| {
            let shape = shape.expect("the program shapes");
            let module = shape.nested_shapes()[0].1;
            let arm = module.nested_shapes()[0].1.arm().expect("an arm");
            assert!(!arm.tail, "a module body is no tail body");
        },
    );
}

#[test]
fn a_unions_payloads_are_type_expressions_and_its_tags_name_nothing() {
    let undefined = refusal("UNION Mb = #{Some: Undefined}");
    assert!(
        matches!(undefined, ShapeError::Unbound { .. }),
        "{undefined:?}"
    );
    shaped(
        "UNION Mb = #{Some: Number, None: Null}",
        |fixture, _, shape| {
            let shape = shape.expect("the program shapes");
            let some = BinderSymbol::Type(type_name("Some", fixture.symbols));
            assert!(shape.mentions().iter().all(|mention| mention.name != some));
        },
    );
}

#[test]
fn a_field_label_is_read_only_when_it_is_no_bare_name() {
    for (label, reads) in [("y", false), ("#(y)", false), ("(which)", true)] {
        let source = format!("LET p = {{y = 1}}\nLET which = #(y)\nLET v = (ATTR p {label})");
        shaped(&source, |fixture, _, shape| {
            let shape = shape.expect("the program shapes");
            let mentioned = |name: &str| {
                let name = BinderSymbol::Value(value_name(name, fixture.symbols));
                shape.mentions().iter().any(|mention| mention.name == name)
            };
            assert_eq!(mentioned("which"), reads, "{label}");
            assert!(!mentioned("y"), "{label}");
        });
    }
}

#[test]
fn a_value_dicts_default_is_refused_and_an_arm_sets_is_not() {
    for source in [
        "LET d = {1: 2, _: 3}",
        "LET f = (FN :{} -> Any = #({1: 2, _: 3}))",
        "LET q = #{_: (3)}",
    ] {
        let error = refusal(source);
        assert!(matches!(error, ShapeError::DictDefault { .. }), "{source}");
        if source == "LET d = {1: 2, _: 3}" {
            assert_eq!(located(error.at()), "{1: 2, _: 3}");
        }
    }
    shaped("MATCH 1 -> :Number WITH #{_: (3)}", |_, _, shape| {
        assert!(shape.is_ok(), "an arm set's `_` is its default arm");
    });
}

#[test]
fn a_group_in_a_quoted_body_is_claimed_and_one_in_a_free_quote_is_not() {
    let group =
        "GROUP g FOLD RIGHT = ((OP #(@) OVER Ring = #(left)) (OP #(%) OVER Ring = #(right)))";
    shaped(
        &format!("LET f = (FN :{{}} -> Any = #(\n  {group}\n  USING g SCOPE (1 @ 2 % 3)\n))"),
        |_, _, shape| {
            assert!(shape.is_ok(), "the body's group chains its run");
        },
    );
    shaped(
        &format!("LET q = #({group})\nLET r = (1 @ 2 @ 3)"),
        |_, _, shape| {
            assert!(shape.is_ok(), "a quote no builtin reads claims nothing");
        },
    );
}

#[test]
fn an_operator_run_in_a_quoted_body_or_an_arm_is_chained() {
    shaped(
        "LET f = (FN :{a :Number} -> Number = #(a + 1 + 2))\nMATCH 1 -> :Number WITH #{Number: (it + 1 + 2)}",
        |_, lines, shape| {
            let shape = shape.expect("the program shapes");
            let chained = |statement: &KExpression<'_>| matches!(statement.parts, [left, _, _] if matches!(left.value, ExpressionPart::Expression(_)));
            assert!(chained(&lambda_body(shape, &lines[0]).body()[0]));
            let (_, arm) = arms(shape, &shape.body()[1], 5)[0];
            assert!(chained(&arm.body()[0]));
        },
    );
}
