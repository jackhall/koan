//! A keyworded use in a quote's code: the candidates an unmarked use, a `$(…)` and a `\(…)` each
//! resolve, a group mark over an operator run, the `NOT` a `!=` becomes, the keyworded holes the
//! code leaves open and which of them it requires, and the keys an `EVAL` offers.

use crate::parse::{ExpressionPart, KExpression, Mark};
use crate::scope::{
    BodyShape, Builtins, Candidate, CandidateList, CaptureSlot, CaptureSource, Coordinate, Offer,
    ShapeError, ShapeKind, Site, Target,
};
use crate::symbols::BinderSymbol;
use crate::type_lattice::{KType, display_name};

use super::{Fixture, builtins, located, with_fixture};

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
                fixture.symbols,
                fixture.scratch(),
            );
            check(fixture, shape)
        })
    })
}

/// Build `source`, which must shape, and hand the shape to `check`.
fn built<R>(
    source: &str,
    check: impl for<'f, 'graph> FnOnce(&Fixture<'f, 'graph>, &'graph BodyShape<'graph>) -> R,
) -> R {
    shaped(source, |fixture, shape| {
        let shape = shape.unwrap_or_else(|error| {
            panic!(
                "`{source}` shapes: {}",
                error.display(fixture.symbols, fixture.types)
            )
        });
        check(fixture, shape)
    })
}

/// The code shape of the quote a `LET <name> = #(…)` statement at `statement` binds.
fn code<'graph>(shape: &BodyShape<'graph>, statement: usize) -> &'graph BodyShape<'graph> {
    let code = shape
        .nested(Site::of(&shape.body()[statement].parts[3].value))
        .expect("a quote value's code is shaped");
    assert_eq!(code.kind(), ShapeKind::Code);
    code
}

/// The node a parenthesized or marked part holds.
fn node<'graph>(part: &ExpressionPart<'graph>) -> &'graph KExpression<'graph> {
    match part {
        ExpressionPart::Expression(node) | ExpressionPart::MarkedUse(_, node) => node.reference(),
        _ => panic!("a node"),
    }
}

/// The candidates of the keyworded use `node` in `shape`.
fn candidates<'graph>(
    shape: &BodyShape<'graph>,
    node: &KExpression<'graph>,
) -> &'graph CandidateList<'graph> {
    shape
        .candidates(Site::of_node(node))
        .expect("a keyworded use has a candidate list")
}

/// The capture `index` of the reader's own activation.
fn captured(index: u32) -> Coordinate {
    Coordinate::Activation {
        hops: 0,
        target: Target::Capture(CaptureSlot(index)),
    }
}

fn key(fixture: &Fixture<'_, '_>, text: &str) -> BinderSymbol {
    BinderSymbol::Key(fixture.symbols.key(text).expect("a key"))
}

/// Whether every candidate is a builtin overload.
fn builtin_only(list: &CandidateList<'_>) -> bool {
    !list.candidates.is_empty()
        && list
            .candidates
            .iter()
            .all(|candidate| matches!(candidate, Candidate::One(Coordinate::Builtin(_))))
}

/// The captures of `shape` that a keyworded use made: its registrations' and its keys'.
fn keyed_captures(shape: &BodyShape<'_>) -> Vec<(BinderSymbol, Option<Mark>)> {
    shape
        .captures()
        .iter()
        .filter(|capture| {
            matches!(
                capture.name,
                BinderSymbol::Registration(_) | BinderSymbol::Key(_)
            )
        })
        .map(|capture| (capture.name, capture.mark))
        .collect()
}

/// The refusal building `source` meets: its location's text and its rendering past the location.
fn refusal(source: &str) -> (String, String) {
    shaped(source, |fixture, shape| {
        let error = shape
            .err()
            .unwrap_or_else(|| panic!("`{source}` is refused"));
        let rendered = error.display(fixture.symbols, fixture.types).to_string();
        let (_, message) = rendered
            .split_once(": ")
            .expect("a diagnostic leads with its location");
        (located(error.at()), message.to_string())
    })
}

#[test]
fn an_unmarked_use_lists_the_builtins_its_codes_registrations_and_its_hole() {
    let source = "EXPR #(GREET x :Any) -> Any = #(x)
LET q = #((EXPR #(GREET y :Any) -> Any = #(y)) (GREET 1) (PRINT 2))";
    built(source, |fixture, shape| {
        let code = code(shape, 1);
        let own = code.registrations()[0].slot;
        // The program's `GREET` is visible where the quote is written, and is no candidate.
        let greet = candidates(code, &code.body()[1]);
        assert_eq!(
            greet.candidates,
            [
                Candidate::One(Coordinate::Activation {
                    hops: 0,
                    target: Target::Local(own)
                }),
                Candidate::Spread(captured(0)),
            ]
        );
        let print = candidates(code, &code.body()[2]);
        assert!(matches!(
            print.candidates,
            [
                Candidate::One(Coordinate::Builtin(_)),
                Candidate::Spread(hole)
            ] if *hole == captured(1)
        ));
        let holes: Vec<_> = code
            .captures()
            .iter()
            .map(|capture| (capture.name, capture.mark, capture.source))
            .collect();
        assert_eq!(
            holes,
            [
                (key(fixture, "GREET _"), None, CaptureSource::Hole),
                (key(fixture, "PRINT _"), None, CaptureSource::Hole),
            ]
        );
        // Each use has a candidate beside its hole, so neither hole is required.
        assert!(code.required_holes().is_empty());
        // A hole is no `\` mark: the quote's type needs nothing.
        assert_eq!(code.code_type(), KType::BLOCK);
    });
}

#[test]
fn a_hole_a_use_selects_from_alone_is_required() {
    built(
        "LET q = #((GREET 1) (GREET 2) (PRINT 3))",
        |fixture, shape| {
            let code = code(shape, 0);
            let greet = candidates(code, &code.body()[0]);
            assert_eq!(greet.candidates, [Candidate::Spread(captured(0))]);
            // Two uses at one key share one hole.
            assert_eq!(code.captures().len(), 2);
            let BinderSymbol::Key(greet) = key(fixture, "GREET _") else {
                unreachable!()
            };
            assert_eq!(code.required_holes(), [greet]);
        },
    );
}

#[test]
fn a_written_use_resolves_where_the_quote_is_written() {
    let source = "EXPR #(GREET x :Any) -> Any = #(x)
LET q = #(PRINT $(GREET 1) $(PRINT 2))";
    built(source, |fixture, shape| {
        let registration = shape.registrations()[0].slot;
        let code = code(shape, 1);
        let parts = code.body()[0].parts;
        let greet = candidates(code, node(&parts[1].value));
        assert_eq!(greet.candidates, [Candidate::One(captured(1))]);
        let capture = code.captures()[1];
        assert_eq!(capture.name, shape.slot_name(registration));
        assert_eq!(capture.mark, Some(Mark::Written));
        assert_eq!(
            capture.source,
            CaptureSource::Read(Coordinate::Activation {
                hops: 0,
                target: Target::Local(registration),
            })
        );
        // A written use is no hole: only the unmarked `PRINT` leaves one.
        assert!(builtin_only(candidates(code, node(&parts[2].value))));
        let names: Vec<_> = code.captures().iter().map(|capture| capture.name).collect();
        assert_eq!(names, [key(fixture, "PRINT _ _"), capture.name]);
    });
    for source in [
        "LET q = #(PRINT $(GREET 1))",
        // The code's own registration is not where the quote is written.
        "LET q = #((EXPR #(GREET y :Any) -> Any = #(y)) (PRINT $(GREET 1)))",
        // A code that cannot be built still resolves its `$` uses where it is written.
        "LET q = #((LET x = 1) (LET x = 2) (PRINT $(GREET 1)))",
    ] {
        let (at, message) = refusal(source);
        assert_eq!(
            message, "`GREET _` has no overload visible here",
            "`{source}`"
        );
        assert_eq!(at, "(GREET 1)", "`{source}`");
    }
}

#[test]
fn a_built_use_is_its_key_offered_where_the_code_is_built() {
    built(
        "LET q = #(PRINT \\(GREET 1) \\(PRINT 2))",
        |fixture, shape| {
            let code = code(shape, 0);
            let parts = code.body()[0].parts;
            let greet = candidates(code, node(&parts[1].value));
            assert_eq!(greet.candidates, [Candidate::Spread(captured(1))]);
            // Not even a builtin: the `EVAL` offers the key's functions, builtins among them.
            let print = candidates(code, node(&parts[2].value));
            assert_eq!(print.candidates, [Candidate::Spread(captured(2))]);
            let captures: Vec<_> = code
                .captures()
                .iter()
                .map(|capture| (capture.name, capture.mark, capture.source))
                .collect();
            let built = Some(Mark::Built);
            assert_eq!(
                captures,
                [
                    (key(fixture, "PRINT _ _"), None, CaptureSource::Hole),
                    (key(fixture, "GREET _"), built, CaptureSource::Offered),
                    (key(fixture, "PRINT _"), built, CaptureSource::Offered),
                ]
            );
            let needing = fixture.types.code_needing(
                fixture.scratch(),
                KType::EXPRESSION,
                &[key(fixture, "GREET _"), key(fixture, "PRINT _")],
            );
            assert_eq!(code.code_type(), needing);
            let rendered =
                display_name(code.code_type(), fixture.types, fixture.symbols).to_string();
            assert!(rendered.contains("(GREET _)"), "{rendered}");
        },
    );
    // A refused code's type still needs the key.
    built(
        "LET q = #((LET x = 1) (LET x = 2) (PRINT \\(GREET 1)))",
        |fixture, shape| {
            let needing = fixture.types.code_needing(
                fixture.scratch(),
                KType::BLOCK,
                &[key(fixture, "GREET _")],
            );
            assert_eq!(code(shape, 0).code_type(), needing);
        },
    );
}

#[test]
fn a_built_use_lists_its_codes_registrations_beside_its_offered_key() {
    for (source, registered) in [
        (
            "LET q = #((EXPR #(GREET y :Any) -> Any = #(y)) (PRINT \\(GREET 1)))",
            true,
        ),
        // A registration written after the use is not seen by it.
        (
            "LET q = #((PRINT \\(GREET 1)) (EXPR #(GREET y :Any) -> Any = #(y)))",
            false,
        ),
    ] {
        built(source, |fixture, shape| {
            let code = code(shape, 0);
            let at = usize::from(registered);
            let parts = code.body()[at].parts;
            let greet = candidates(code, node(&parts[1].value));
            let offered = match (registered, greet.candidates) {
                (
                    true,
                    [
                        Candidate::One(Coordinate::Activation {
                            hops: 0,
                            target: Target::Local(own),
                        }),
                        Candidate::Spread(offered),
                    ],
                ) => {
                    assert_eq!(*own, code.registrations()[0].slot, "`{source}`");
                    offered
                }
                (false, [Candidate::Spread(offered)]) => offered,
                (_, listed) => panic!("`{source}` lists {listed:?}"),
            };
            let Coordinate::Activation {
                target: Target::Capture(index),
                ..
            } = offered
            else {
                panic!("`{source}`: an offered key is a capture");
            };
            let capture = code.captures()[index.0 as usize];
            assert_eq!(
                (capture.name, capture.mark, capture.source),
                (
                    key(fixture, "GREET _"),
                    Some(Mark::Built),
                    CaptureSource::Offered
                ),
                "`{source}`"
            );
            // The quote's type still needs the key, as the offered capture does.
            let needing = fixture.types.code_needing(
                fixture.scratch(),
                KType::BLOCK,
                &[key(fixture, "GREET _")],
            );
            assert_eq!(code.code_type(), needing, "`{source}`");
        });
    }
}

/// Every key a `\(…)` leaves open renders as written, even one no source spells: the combiner of
/// a chained comparison, and the `==` of a `!=`.
#[test]
fn each_key_a_built_mark_leaves_open_renders_as_written() {
    for (source, keys) in [
        ("LET q = #(\\(a < b < c))", &["(_ < _)", "(_ AND _)"][..]),
        ("LET q = #(\\(a != b))", &["(_ == _)"][..]),
    ] {
        built(source, |fixture, shape| {
            let code_type = code(shape, 0).code_type();
            let rendered = display_name(code_type, fixture.types, fixture.symbols).to_string();
            for key in keys {
                assert!(rendered.contains(key), "`{source}`: {rendered}");
            }
        });
    }
}

#[test]
fn a_mark_over_an_operator_run_covers_each_use_the_run_becomes() {
    let source = "OP #(<) OVER Number -> Bool = #(left)
LET q = #($(a < b < c))";
    built(source, |_, shape| {
        let registration = shape.registrations()[0].slot;
        let code = code(shape, 1);
        // `a < b < c` chains pairwise: `(a < b) AND (b < c)`.
        let joined = node(&code.body()[0].parts[0].value);
        assert!(
            builtin_only(candidates(code, joined)),
            "the `AND` is marked too"
        );
        for pair in [0, 2] {
            let less = candidates(code, node(&joined.parts[pair].value));
            assert!(matches!(
                less.candidates,
                [Candidate::One(Coordinate::Builtin(_)), Candidate::One(read)] if *read == captured(0)
            ));
        }
        // The operator run's two `<` uses share one capture, and none of its uses is a hole.
        assert_eq!(
            keyed_captures(code),
            [(shape.slot_name(registration), Some(Mark::Written))]
        );
    });
    // An operator run written in an operand is one of its own, which the mark does not cover.
    built("LET q = #($(a + (b * c) + d))", |fixture, shape| {
        let code = code(shape, 0);
        let outer = node(&code.body()[0].parts[0].value);
        let inner = node(&outer.parts[0].value);
        let times = node(&inner.parts[2].value);
        assert!(builtin_only(candidates(code, outer)));
        assert!(builtin_only(candidates(code, inner)));
        assert!(matches!(
            candidates(code, times).candidates,
            [Candidate::One(Coordinate::Builtin(_)), Candidate::Spread(_)]
        ));
        assert_eq!(keyed_captures(code), [(key(fixture, "_ * _"), None)]);
    });
}

#[test]
fn a_mark_over_a_hoisting_run_reaches_the_block_the_run_becomes() {
    built("LET q = #($(a < (FIRST b) < c))", |_, shape| {
        let code = code(shape, 0);
        // The mark holds the operator run's block in a node of one part.
        let holder = node(&code.body()[0].parts[0].value);
        let block = code
            .nested(Site::of(&holder.parts[0].value))
            .expect("the hoisting operator run is a block");
        assert_eq!(block.kind(), ShapeKind::Block);
        let joined = block.body().last().expect("the folded pairs");
        assert!(builtin_only(candidates(block, joined)));
        for pair in [0, 2] {
            assert!(builtin_only(candidates(
                block,
                node(&joined.parts[pair].value)
            )));
        }
        // The hoisted operand is no use of the operator run: `FIRST` stays a hole.
        let hoisted = &block.body()[0];
        let first = node(&hoisted.parts[3].value);
        assert!(matches!(
            candidates(block, first).candidates,
            [Candidate::Spread(_)]
        ));
    });
}

#[test]
fn the_not_of_an_unequal_pair_is_always_the_builtins() {
    let source = "EXPR #(NOT x :Any) -> Any = #(x)
OP #(==) OVER Number = #(left)
LET r = (1 != 2)
LET q = #($(a != b))
LET s = (NOT 1)";
    built(source, |_, shape| {
        let negation = node(&shape.body()[2].parts[3].value);
        assert!(builtin_only(candidates(shape, negation)));
        let equal = candidates(shape, node(&negation.parts[1].value));
        assert_eq!(equal.candidates.len(), 2, "the user's `==` is a candidate");
        // A `NOT` written as such sees the user's.
        let written = candidates(shape, node(&shape.body()[4].parts[3].value));
        assert_eq!(written.candidates.len(), 2);
        // Under a mark, the `==` resolves where the quote is written; the `NOT` stays the builtin's.
        let code = code(shape, 3);
        let negation = node(&code.body()[0].parts[0].value);
        assert!(builtin_only(candidates(code, negation)));
        let equal = candidates(code, node(&negation.parts[1].value));
        assert!(matches!(
            equal.candidates,
            [Candidate::One(Coordinate::Builtin(_)), Candidate::One(read)] if *read == captured(0)
        ));
        let [(registration, Some(Mark::Written))] = keyed_captures(code)[..] else {
            panic!("no hole for either use");
        };
        assert!(matches!(registration, BinderSymbol::Registration(_)));
    });
}

#[test]
fn an_eval_offers_each_needed_key_as_a_use_written_there_lists_it() {
    let source = "EXPR #(GREET x :Any) -> Any = #(x)
LET twice = (FN :{body :(Expression NEEDING #[(GREET _) (PRINT _)])} -> Any = #(EVAL body -> Any))";
    built(source, |fixture, shape| {
        let lambda = node(&shape.body()[1].parts[3].value);
        let body = shape
            .nested(Site::of(&lambda.parts[5].value))
            .expect("the lambda's body");
        let eval = &body.body()[0];
        let offered = body.offers(Site::of(&eval.parts[1].value));
        let keys: Vec<_> = offered.iter().map(|(name, _)| *name).collect();
        assert_eq!(keys, [key(fixture, "GREET _"), key(fixture, "PRINT _")]);
        let [(_, Offer::Key(greet)), (_, Offer::Key(print))] = offered else {
            panic!("two keys are offered: {offered:?}");
        };
        // The body captures the program's registration, as a use written at the `EVAL` would.
        assert_eq!(greet.candidates, [Candidate::One(captured(0))]);
        assert!(builtin_only(print));
    });
    let (at, message) = refusal(
        "LET twice = (FN :{body :(Expression NEEDING #[(GREET _)])} -> Any = #(EVAL body -> Any))",
    );
    assert_eq!(message, "`GREET _` has no overload visible here");
    assert_eq!(at, "(EVAL body -> Any)");
}
