//! The operator-run rewrite: what each of the four chainings builds, where a nested operator run is
//! reached, and which operator runs the builder refuses rather than chains.

use crate::parse::{ExpressionPart, KExpression};
use crate::scope::{BodyShape, Builtins, Position, ShapeError, ShapeKind, Site};
use crate::symbols::{BinderSymbol, KeywordSymbol, SymbolInterner};

use super::{Fixture, builtins, value_name, with_fixture};

/// Build `source` against the suites' builtins and hand the result to `check`.
fn shaped<R>(
    source: &str,
    check: impl for<'f, 'graph> FnOnce(
        &Fixture<'f, 'graph>,
        Result<&'graph BodyShape<'graph>, ShapeError>,
    ) -> R,
) -> R {
    with_fixture(|fixture| {
        // The rewrite mints its own vocabulary through `classify`, which records no text, so a
        // source that never spells `AND`, `NOT` or `==` renders them as the interner's placeholder.
        for name in ["AND", "NOT", "=="] {
            KeywordSymbol::declared(name, fixture.symbols).expect("a keyword token");
        }
        let lines = fixture.parse(source);
        fixture.in_cell(|writer, _| {
            let table: &Builtins = builtins(fixture, writer);
            let shape = BodyShape::of_program(fixture.program, &lines, table, fixture.scratch());
            check(fixture, shape)
        })
    })
}

/// A rewritten statement's surface with every nested node parenthesized, so how the rewrite nested
/// it is legible: `1 + 2 - 3` folds left to `((1 + 2) - 3)`.
fn tree(node: &KExpression<'_>, symbols: &SymbolInterner) -> String {
    let mut out = String::from("(");
    for (index, part) in node.parts.iter().enumerate() {
        if index > 0 {
            out.push(' ');
        }
        out.push_str(&part_tree(&part.value, symbols));
    }
    out.push(')');
    out
}

fn part_tree(part: &ExpressionPart<'_>, symbols: &SymbolInterner) -> String {
    match part {
        ExpressionPart::Expression(node) => tree(node.reference(), symbols),
        ExpressionPart::SigiledTypeExpr(node) => format!(":{}", tree(node.reference(), symbols)),
        ExpressionPart::ListLiteral(items) => {
            let mut out = String::from("[");
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(' ');
                }
                out.push_str(&part_tree(item, symbols));
            }
            out.push(']');
            out
        }
        other => other.summarize(symbols),
    }
}

/// The node a part holds, which must be parenthesized.
fn expression<'graph>(part: &ExpressionPart<'graph>) -> &'graph KExpression<'graph> {
    let ExpressionPart::Expression(node) = part else {
        panic!("the part is a parenthesized node");
    };
    node.reference()
}

/// The shape nested at part `index` of `node`.
fn nested<'graph>(
    shape: &BodyShape<'graph>,
    node: &KExpression<'graph>,
    index: usize,
) -> &'graph BodyShape<'graph> {
    shape
        .nested(Site::of(&node.parts[index].value))
        .expect("the part has a nested shape")
}

fn value(fixture: &Fixture<'_, '_>, text: &str) -> BinderSymbol {
    BinderSymbol::Value(value_name(text, fixture.symbols))
}

fn keyword(text: &str, symbols: &SymbolInterner) -> KeywordSymbol {
    KeywordSymbol::declared(text, symbols).expect("a keyword token")
}

#[test]
fn a_builtin_group_folds_left_and_a_declared_one_folds_the_way_it_says() {
    shaped("1 + 2 - 3", |fixture, shape| {
        let shape = shape.expect("the program shapes");
        assert_eq!(tree(&shape.body()[0], fixture.symbols), "((1 + 2) - 3)");
    });
    // A declared group is visible inside its own body, and nests the way its mode says.
    let source = "\
GROUP ring FOLD RIGHT = (\
 (OP #(@) OVER Ring = (left))\
 (LET x = (1 @ 2 @ 3)))";
    shaped(source, |fixture, shape| {
        let shape = shape.expect("the program shapes");
        let body = nested(shape, &shape.body()[0], 5);
        assert_eq!(
            tree(expression(&body.body()[1].parts[3].value), fixture.symbols),
            "(1 @ (2 @ 3))"
        );
    });
}

#[test]
fn a_unary_group_calls_one_keyword_over_a_list_of_operands() {
    shaped("LET Either = :(Number | Str | Null)", |fixture, shape| {
        let shape = shape.expect("the program shapes");
        assert_eq!(
            part_tree(&shape.body()[0].parts[3].value, fixture.symbols),
            ":(| [Number Str Null])"
        );
    });
}

#[test]
fn a_symbol_no_group_claims_folds_left_alone_in_every_body_that_declares_it() {
    shaped(
        "(OP #(@) OVER Ring = (left))\nLET x = (1 @ 2 @ 3)",
        |fixture, shape| {
            let shape = shape.expect("the program shapes");
            assert_eq!(
                tree(expression(&shape.body()[1].parts[3].value), fixture.symbols),
                "((1 @ 2) @ 3)"
            );
        },
    );
    // A bare `OP` declares no group, so two siblings over one symbol never collide.
    let siblings = "\
MODULE first = ((OP #(@) OVER Ring = (left)) (LET a = (1 @ 2 @ 3)))
MODULE second = ((OP #(@) OVER Ring = (left)) (LET b = (1 @ 2 @ 3)))";
    shaped(siblings, |fixture, shape| {
        let shape = shape.expect("the program shapes");
        for statement in 0..2 {
            let body = nested(shape, &shape.body()[statement], 3);
            assert_eq!(
                tree(expression(&body.body()[1].parts[3].value), fixture.symbols),
                "((1 @ 2) @ 3)"
            );
        }
    });
}

#[test]
fn an_operator_run_is_chained_wherever_it_is_written_and_a_quote_is_left_alone() {
    let source = "\
LET a = [(1 + 2 - 3)]
LET b = (1 + 2 - 3)
LET c = (origin (1 + 2 - 3))
LET d = (1 + (2 * 3 * 4) - 5)
LET e = #(1 + 2 - 3)
LET f = (FN :{} -> Number = (1 + 2 - 3))";
    shaped(source, |fixture, shape| {
        let shape = shape.expect("the program shapes");
        let rendered: Vec<String> = shape
            .body()
            .iter()
            .map(|statement| part_tree(&statement.parts[3].value, fixture.symbols))
            .collect();
        assert_eq!(rendered[0], "[((1 + 2) - 3)]");
        assert_eq!(rendered[1], "((1 + 2) - 3)");
        assert_eq!(rendered[2], "(origin ((1 + 2) - 3))");
        assert_eq!(rendered[3], "((1 + ((2 * 3) * 4)) - 5)");
        assert_eq!(rendered[4], "#(1 + 2 - 3)", "a quote is data");

        // The rewritten part is the one the shape hands a reader for the binding's right-hand side.
        let (slot, _) = shape.slot(value(fixture, "b")).expect("`b` is bound");
        let rhs = shape
            .rhs(slot)
            .expect("a `LET` records its right-hand side");
        assert!(std::ptr::eq(rhs, &shape.body()[1].parts[3].value));

        // A body is rewritten by its own draft, under its own frame.
        let body = nested(shape, expression(&shape.body()[5].parts[3].value), 5);
        assert_eq!(tree(&body.body()[0], fixture.symbols), "((1 + 2) - 3)");
    });
}

#[test]
fn a_pairwise_group_compares_adjacent_operands_and_hoists_the_shared_ones() {
    shaped("1 < 2 < 3", |fixture, shape| {
        let shape = shape.expect("the program shapes");
        assert_eq!(
            tree(&shape.body()[0], fixture.symbols),
            "((1 < 2) AND (2 < 3))"
        );
        assert!(
            shape.nested_shapes().is_empty(),
            "operands that evaluate once are never hoisted"
        );
    });

    shaped("LET zz = 1\n1 < (zz) <= 3", |fixture, shape| {
        let shape = shape.expect("the program shapes");
        let statement = &shape.body()[1];
        assert_eq!(statement.parts.len(), 1, "the block is held by one part");
        let block = nested(shape, statement, 0);
        assert_eq!(block.kind(), ShapeKind::Block);
        assert_eq!(block.slots(), 1, "one operand is hoisted");
        assert_eq!(block.body().len(), 2, "the hoist, then the comparison");
        let rendered = tree(expression(&statement.parts[0].value), fixture.symbols);
        assert_eq!(
            rendered.matches("zz").count(),
            1,
            "a hoisted operand is written once: {rendered}"
        );
    });

    // Pairs fold through the group's combiner, in the group's direction.
    let source = "\
GROUP cmp PAIRWISE FOLD #(AND) RIGHT = (\
 (OP #(~) OVER Ring -> Bool = (left))\
 (LET x = (1 ~ 2 ~ 3 ~ 4)))";
    shaped(source, |fixture, shape| {
        let shape = shape.expect("the program shapes");
        let body = nested(shape, &shape.body()[0], 7);
        assert_eq!(
            tree(expression(&body.body()[1].parts[3].value), fixture.symbols),
            "((1 ~ 2) AND ((2 ~ 3) AND (3 ~ 4)))"
        );
    });
}

#[test]
fn equality_joins_a_pairwise_group_and_the_unequal_symbol_is_never_built() {
    shaped("1 == 2 == 3", |fixture, shape| {
        let shape = shape.expect("the program shapes");
        assert_eq!(
            tree(&shape.body()[0], fixture.symbols),
            "((1 == 2) AND (2 == 3))"
        );
    });
    shaped("1 < 2 != 3", |fixture, shape| {
        let shape = shape.expect("the program shapes");
        assert_eq!(
            tree(&shape.body()[0], fixture.symbols),
            "((1 < 2) AND (NOT (2 == 3)))"
        );
    });
    shaped("LET x = (1 != 2)\nLET y = #(1 != 2)", |fixture, shape| {
        let shape = shape.expect("the program shapes");
        assert_eq!(
            part_tree(&shape.body()[0].parts[3].value, fixture.symbols),
            "(NOT (1 == 2))"
        );
        assert_eq!(
            part_tree(&shape.body()[1].parts[3].value, fixture.symbols),
            "#(1 != 2)"
        );
    });
    // Beside a declared pairwise group's member, equality folds through that group's combiner.
    let source = "\
GROUP cmp PAIRWISE FOLD #(AND) LEFT = (\
 (OP #(~) OVER Ring -> Bool = (left))\
 (LET x = (1 ~ 2 == 3)))";
    shaped(source, |fixture, shape| {
        let shape = shape.expect("the program shapes");
        let body = nested(shape, &shape.body()[0], 7);
        assert_eq!(
            tree(expression(&body.body()[1].parts[3].value), fixture.symbols),
            "((1 ~ 2) AND (2 == 3))"
        );
    });
}

#[test]
fn an_operator_run_no_one_chaining_covers_is_refused() {
    let unchained = "\
GROUP ring FOLD LEFT = ((OP #(@) OVER Ring = (left)))
LET x = (1 @ 2 @ 3)";
    shaped(unchained, |fixture, shape| {
        assert_eq!(
            shape.err(),
            Some(ShapeError::Unchained {
                symbol: keyword("@", fixture.symbols),
                at: Position::statement(1),
            })
        );
    });
    shaped("1 + 2 * 3", |fixture, shape| {
        assert_eq!(
            shape.err(),
            Some(ShapeError::MixedGroups {
                first: keyword("+", fixture.symbols),
                second: keyword("*", fixture.symbols),
                at: Position::statement(0),
            })
        );
    });
    shaped("1 + 2 == 3", |_, shape| {
        assert!(matches!(shape.err(), Some(ShapeError::MixedGroups { .. })));
    });
    shaped("(OP #(@) OVER Ring -> Ring = (left))", |fixture, shape| {
        assert_eq!(
            shape.err(),
            Some(ShapeError::ResultOutsidePairwise {
                symbol: keyword("@", fixture.symbols),
                at: Position::statement(0),
            })
        );
    });
    shaped("LET x = (origin :| origin :| origin)", |fixture, shape| {
        assert_eq!(
            shape.err(),
            Some(ShapeError::SpellsForm {
                symbol: keyword(":|", fixture.symbols),
                at: Position::statement(0),
            })
        );
    });
    shaped("(OP #(!=) OVER Ring -> Bool = (left))", |fixture, shape| {
        assert_eq!(
            shape.err(),
            Some(ShapeError::Derived {
                symbol: keyword("!=", fixture.symbols),
                at: Position::PARAMETER,
            })
        );
    });
}
