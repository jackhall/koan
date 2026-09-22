//! The operator-run rewrite: what each of the four chainings builds, where a nested operator run is
//! reached, which bodies a declared group reaches through `USING` and `EVAL`, and which operator
//! runs the builder refuses rather than chains.

use crate::memory::resident;
use crate::parse::{ExpressionPart, KExpression};
use crate::scope::{Activation, BodyShape, Builtins, Position, ShapeError, ShapeKind, Site};
use crate::symbols::{BinderSymbol, KeywordSymbol, SymbolInterner};

use super::{Fixture, Probe, ProbeFamily, builtins, value_name, with_fixture};

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
        fixture.in_cell(|writer| {
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

/// Shape `quoted` as the body of an `EVAL`, at the program of `source` or — when `inside_using` —
/// in the `USING` body its last statement opens.
fn evaluated<R>(
    source: &str,
    quoted: &str,
    inside_using: bool,
    check: impl for<'f, 'graph> FnOnce(
        &Fixture<'f, 'graph>,
        Result<&'graph BodyShape<'graph>, ShapeError>,
    ) -> R,
) -> R {
    with_fixture(|fixture| {
        for name in ["AND", "NOT", "=="] {
            KeywordSymbol::declared(name, fixture.symbols).expect("a keyword token");
        }
        let lines = fixture.parse(source);
        let quoting = fixture.parse(quoted);
        let ExpressionPart::QuotedExpression(quote) = quoting[0].parts[0].value else {
            panic!("`{quoted}` is a quote");
        };
        let quote = quote.reference();
        fixture.in_cell(|writer| {
            let table: &Builtins<'_, '_, Probe> = builtins(fixture, writer);
            let shape = BodyShape::of_program(fixture.program, &lines, table, fixture.scratch())
                .expect("the program shapes");
            let program: &Activation<'_, '_, ProbeFamily> =
                resident(writer, Activation::of_program(writer, shape, table));
            let (site, at) = if inside_using {
                let using = &shape.body()[shape.body().len() - 1];
                let block = nested(shape, using, 3);
                let site = resident(writer, Activation::of_block(writer, block, program));
                (site, Position::statement(0))
            } else {
                (program, shape.end())
            };
            check(
                fixture,
                BodyShape::for_eval(fixture.program, quote, site, at, fixture.scratch()),
            )
        })
    })
}

/// A `GROUP` whose members are `@` and `&`, under `mode`, named `name`.
fn group(name: &str, mode: &str) -> String {
    format!("GROUP {name} {mode} = ((OP #(@) OVER Ring = (left)) (OP #(&) OVER Ring = (right)))")
}

/// A `SIG` named `name` whose body is one bodyless `GROUP` head over `members`, under `mode`.
fn signature(name: &str, mode: &str, members: &[&str]) -> String {
    let heads: Vec<String> = members
        .iter()
        .map(|symbol| format!("(OP #({symbol}) OVER Ring)"))
        .collect();
    format!("SIG {name} = ((GROUP {mode} = ({})))", heads.join(" "))
}

#[test]
fn a_using_body_chains_the_group_its_operand_surfaces() {
    let source = format!(
        "{}
USING g SCOPE (1 @ 2 & 3)",
        group("g", "FOLD RIGHT")
    );
    shaped(&source, |fixture, shape| {
        let shape = shape.expect("the program shapes");
        let block = nested(shape, &shape.body()[1], 3);
        assert_eq!(block.kind(), ShapeKind::Block);
        assert_eq!(tree(&block.body()[0], fixture.symbols), "(1 @ (2 & 3))");
    });

    // A signature's bodyless `GROUP` head is a group too, reached through an ascription.
    let module = "MODULE m = ((OP #(@) OVER Ring = (left)) (OP #(&) OVER Ring = (right)))";
    let source = format!(
        "{}
{module}
USING (m :! Ops) SCOPE (1 @ 2 & 3)",
        signature("Ops", "FOLD LEFT", &["@", "&"])
    );
    shaped(&source, |fixture, shape| {
        let shape = shape.expect("the program shapes");
        let block = nested(shape, &shape.body()[2], 3);
        assert_eq!(tree(&block.body()[0], fixture.symbols), "((1 @ 2) & 3)");
    });
}

#[test]
fn a_group_surfaced_twice_is_held_once_and_a_second_chaining_is_refused() {
    let module = "MODULE m = ((OP #(@) OVER Ring = (left)) (OP #(&) OVER Ring = (right)))";
    let nest = |first: &str, second: &str| {
        format!(
            "{first}
{second}
{module}
USING (m :! Ops) SCOPE ((USING (m :! Peer) SCOPE (1 @ 2 & 3)))"
        )
    };
    let equal = nest(
        &signature("Ops", "FOLD LEFT", &["@", "&"]),
        &signature("Peer", "FOLD LEFT", &["@", "&"]),
    );
    shaped(&equal, |fixture, shape| {
        let shape = shape.expect("two equal groups are one group");
        let outer = nested(shape, &shape.body()[3], 3);
        let inner = nested(outer, &outer.body()[0], 3);
        assert!(
            inner.held_groups().is_empty(),
            "the group the outer body holds is not held twice"
        );
        assert_eq!(tree(&inner.body()[0], fixture.symbols), "((1 @ 2) & 3)");
    });

    let unequal = nest(
        &signature("Ops", "FOLD LEFT", &["@", "&"]),
        &signature("Peer", "FOLD RIGHT", &["@", "&"]),
    );
    shaped(&unequal, |fixture, shape| {
        let members = [keyword("@", fixture.symbols), keyword("&", fixture.symbols)];
        assert!(
            matches!(
                shape.err(),
                Some(ShapeError::RedeclaresGroup { symbol, at })
                    if members.contains(&symbol) && at == Position::statement(0)
            ),
            "a group over one member's other chaining is refused where it is surfaced"
        );
    });

    // Nothing surfaces a second chaining over a builtin group's member.
    let over_builtin = format!(
        "{}
{module}
USING (m :! Ops) SCOPE (1)",
        signature("Ops", "FOLD LEFT", &["+"])
    );
    shaped(&over_builtin, |fixture, shape| {
        assert_eq!(
            shape.err(),
            Some(ShapeError::RedeclaresGroup {
                symbol: keyword("+", fixture.symbols),
                at: Position::statement(2),
            })
        );
    });
}

#[test]
fn evaluated_code_chains_under_its_site_and_is_held_to_the_programs_claims() {
    let source = format!(
        "{}
USING g SCOPE (1)",
        group("g", "FOLD RIGHT")
    );
    evaluated(&source, "#(1 @ 2 & 3)", true, |fixture, shape| {
        let shape = shape.expect("the quoted operator run shapes at its site");
        assert_eq!(tree(&shape.body()[0], fixture.symbols), "(1 @ (2 & 3))");
    });
    // The same operator run outside that body has no group to chain under.
    evaluated(&source, "#(1 @ 2 & 3)", false, |fixture, shape| {
        assert_eq!(
            shape.err(),
            Some(ShapeError::Unchained {
                symbol: keyword("@", fixture.symbols),
                at: Position::statement(0),
            })
        );
    });
    // Evaluated code declares against the program's claims, so it may not re-chain a symbol.
    let redeclared = format!("#({})", group("h", "FOLD RIGHT"));
    evaluated(&group("g", "FOLD LEFT"), &redeclared, false, |_, shape| {
        assert!(matches!(
            shape.err(),
            Some(ShapeError::RedeclaresGroup { .. })
        ));
    });
}

#[test]
fn a_result_typed_operator_is_admitted_wherever_its_group_chains_pairwise() {
    // A signature's bodyless `GROUP` claims nothing — it reaches a body only as a group a `USING`
    // surfaces — so the frame is the only place this declaration's admission can be read off.
    let sig = "SIG Cmp = ((GROUP PAIRWISE FOLD #(AND) LEFT = ((OP #(~) OVER Ring -> Bool))))";
    let module = "MODULE m = ((LET x = 1))";
    let declaration = "(OP #(~) OVER Number -> Bool = (left))";
    shaped(
        &format!("{sig}\n{module}\nUSING (m :! Cmp) SCOPE ({declaration})"),
        |_, shape| {
            shape.expect("the surfaced group chains `~` pairwise, so the result type is admitted");
        },
    );
    // Outside that body nothing holds the group, so the same declaration has no pairwise chaining.
    shaped(
        &format!("{sig}\n{module}\n{declaration}"),
        |fixture, shape| {
            assert_eq!(
                shape.err(),
                Some(ShapeError::ResultOutsidePairwise {
                    symbol: keyword("~", fixture.symbols),
                    at: Position::statement(2),
                })
            );
        },
    );
}
