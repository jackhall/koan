//! What the builder resolves for dispatch: a definition's registration as a slot of its body, each
//! bucket declaration's ranking and the ranking each registration carries, the candidate list of
//! every keyworded use, and the refusals a keyworded definition, declaration or use meets.

use crate::parse::builtin_shapes::BuiltinShapeId;
use crate::parse::{ExpressionPart, KExpression, KeyElement};
use crate::scope::{
    BodyShape, Builtins, Candidate, CandidateList, CaptureSource, Coordinate, Position, ShapeError,
    ShapeKind, Site, Slot, Target, Which,
};
use crate::symbols::{BinderSymbol, KeywordSymbol};

use super::{Fixture, builtins, located, value_name, with_fixture};

/// Build `source` against the suites' builtins and hand the result to `check`.
fn shaped<R>(
    source: &str,
    check: impl for<'f, 'graph> FnOnce(
        &Fixture<'f, 'graph>,
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

/// The refusal building `source` meets, its location's text, and its rendering past the location.
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

/// The node a parenthesized part holds.
fn node<'graph>(part: &ExpressionPart<'graph>) -> &'graph KExpression<'graph> {
    let ExpressionPart::Expression(node) = part else {
        panic!("a parenthesized node");
    };
    node.reference()
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

/// The slot the value name `text` is declared at.
fn slot_of(fixture: &Fixture<'_, '_>, shape: &BodyShape<'_>, text: &str) -> Slot {
    let name = BinderSymbol::Value(value_name(text, fixture.symbols));
    shape.slot(name).expect("a declared name").0
}

/// The body the binder `text` births.
fn birth<'graph>(
    fixture: &Fixture<'_, 'graph>,
    shape: &BodyShape<'graph>,
    text: &str,
) -> &'graph BodyShape<'graph> {
    shape
        .births(slot_of(fixture, shape, text))
        .expect("the binder births a body")
}

fn keyword(fixture: &Fixture<'_, '_>, text: &str) -> KeyElement {
    KeyElement::Keyword(KeywordSymbol::declared(text, fixture.symbols).expect("a keyword"))
}

/// A local read `hops` activations out.
fn local(hops: u32, slot: Slot) -> Candidate {
    Candidate::One(Coordinate::Activation {
        hops,
        target: Target::Local(slot),
    })
}

#[test]
fn a_registration_takes_a_slot_after_every_name() {
    let source = "LET a = 1\nEXPR #(GREET x :Number) -> Number = #(x)\nLET Tn = Number";
    built(source, |fixture, shape| {
        assert_eq!(shape.slots(), 3);
        let [registration] = shape.registrations() else {
            panic!("one registration");
        };
        assert_eq!(
            registration.slot,
            Slot(2),
            "values, types, then registrations"
        );
        assert!(matches!(
            shape.slot_name(registration.slot),
            BinderSymbol::Registration(_)
        ));
        assert_eq!(
            shape.slot(shape.slot_name(registration.slot)),
            Some((registration.slot, Position::statement(1)))
        );
        assert_eq!(registration.key, fixture.symbols.key("GREET _").unwrap());
        assert_eq!(
            registration.elements,
            [keyword(fixture, "GREET"), KeyElement::Slot]
        );
        assert_eq!(registration.classes, [0]);
        assert_eq!(registration.which, Which::Only);
        assert_eq!(
            shape
                .registration(registration.slot)
                .map(|found| found.slot),
            Some(Slot(2))
        );
        let body = shape
            .births(registration.slot)
            .expect("a definition births its body");
        assert_eq!(body.kind(), ShapeKind::Callable);
        assert!(shape.registration(Slot(0)).is_none());
    });
}

#[test]
fn a_statement_reads_a_registration_written_before_it_and_no_later_one() {
    let source = "EXPR #(GREET x :Number) -> Number = #(x)\nLET a = (GREET 1)";
    built(source, |_, shape| {
        let registration = shape.registrations()[0].slot;
        let list = candidates(shape, node(&shape.body()[1].parts[3].value));
        assert_eq!(list.candidates, [local(0, registration)]);
        assert_eq!(list.classes, [0]);
        // The definition is performed first: its reader waits on it.
        assert_eq!(shape.units().len(), 2);
    });
    let (at, message) = refusal("LET a = (GREET 1)\nEXPR #(GREET x :Number) -> Number = #(x)");
    assert_eq!(message, "`GREET _` has no overload visible here");
    assert_eq!(at, "(GREET 1)");
}

#[test]
fn a_use_in_a_callable_body_captures_a_later_registration() {
    let source = "LET f = (FN :{} -> Number = #(GREET 1))\n\
                  EXPR #(GREET x :Number) -> Number = #(x)";
    built(source, |fixture, shape| {
        let registration = shape.registrations()[0].slot;
        let body = birth(fixture, shape, "f");
        let list = candidates(body, &body.body()[0]);
        assert_eq!(
            list.candidates,
            [Candidate::One(Coordinate::Activation {
                hops: 0,
                target: Target::Capture(crate::scope::CaptureSlot(0)),
            })]
        );
        let [capture] = body.captures() else {
            panic!("one capture");
        };
        assert_eq!(capture.name, shape.slot_name(registration));
        assert_eq!(
            capture.source,
            CaptureSource::Read(Coordinate::Activation {
                hops: 0,
                target: Target::Local(registration),
            })
        );
    });
}

#[test]
fn mutually_recursive_registrations_are_one_component() {
    let source = "EXPR #(PING x :Number) -> Number = #(PONG x)\n\
                  EXPR #(PONG x :Number) -> Number = #(PING x)";
    built(source, |_, shape| {
        let [ping, pong] = shape.registrations() else {
            panic!("two registrations");
        };
        assert_eq!(
            shape.component_index(ping.slot),
            shape.component_index(pong.slot)
        );
        let component = shape.component_of(ping.slot);
        assert!(component.cyclic && component.deferred_only);
        assert_eq!(shape.units().len(), 1);
    });
}

#[test]
fn a_combined_statement_binds_its_name_and_each_registration_in_one_component() {
    built(
        "LET twice = FN EXPR #(TWICE x :Number) -> Number = #(x)",
        |fixture, shape| {
            let name = slot_of(fixture, shape, "twice");
            let [registration] = shape.registrations() else {
                panic!("one registration");
            };
            assert_eq!(shape.slots(), 2);
            assert_eq!(
                shape.component_index(name),
                shape.component_index(registration.slot)
            );
            let body = shape.births(name).expect("the name births the body");
            assert!(std::ptr::eq(
                body,
                shape
                    .births(registration.slot)
                    .expect("so does the registration")
            ));
            assert_eq!(shape.units().len(), 1);
        },
    );
    built(
        "LET negate = UNARY OP #(~) OVER Number -> Number = #(operands)",
        |fixture, shape| {
            let name = slot_of(fixture, shape, "negate");
            let [first, second] = shape.registrations() else {
                panic!("a unary operator registers under two keys");
            };
            let mut keys = [(first.which, first.classes), (second.which, second.classes)];
            keys.sort_by_key(|(which, _)| *which as u8);
            assert_eq!(
                keys,
                [(Which::Unary, &[0][..]), (Which::Binary, &[0, 1][..])]
            );
            for registration in [first, second] {
                assert_eq!(
                    shape.component_index(registration.slot),
                    shape.component_index(name)
                );
            }
        },
    );
}

#[test]
fn a_candidate_list_keys_on_the_full_bucket_key() {
    let definition = "EXPR #(MOVE x :Number TO y :Number) -> Number = #(x)";
    let (_, message) = refusal(&format!("{definition}\nLET a = (MOVE 1)"));
    assert_eq!(message, "`MOVE _` has no overload visible here");
    built(
        &format!("{definition}\nLET b = (MOVE 1 TO 2)"),
        |_, shape| {
            let list = candidates(shape, node(&shape.body()[1].parts[3].value));
            assert_eq!(list.candidates.len(), 1);
        },
    );
}

#[test]
fn a_lone_keyword_is_a_use_of_its_key() {
    built("EXPR #(NOW) -> Number = #(1)\nLET a = (NOW)", |_, shape| {
        let registration = shape.registrations()[0].slot;
        let list = candidates(shape, node(&shape.body()[1].parts[3].value));
        assert_eq!(list.candidates, [local(0, registration)]);
    });
    let (at, message) = refusal("LET a = (NOW)");
    assert_eq!(message, "`NOW` has no overload visible here");
    assert_eq!(at, "(NOW)");
}

#[test]
fn builtin_overloads_come_first_in_a_candidate_list() {
    let source = "EXPR #(PRINT x :Str) -> Str = #(x)\nLET a = (PRINT \"s\")";
    built(source, |_, shape| {
        let registration = shape.registrations()[0].slot;
        let list = candidates(shape, node(&shape.body()[1].parts[3].value));
        let [Candidate::One(Coordinate::Builtin(_)), user] = list.candidates else {
            panic!("the builtin overload, then the registration");
        };
        assert_eq!(*user, local(0, registration));
    });
}

#[test]
fn a_block_reads_an_outer_registration_and_keeps_its_own() {
    let source = "EXPR #(GREET x :Number) -> Number = #(x)\n\
                  MATCH 1 -> :Number WITH #{Number: (GREET it)}";
    built(source, |_, shape| {
        let registration = shape.registrations()[0].slot;
        let ExpressionPart::DictLiteral(arms) = &shape.body()[1].parts[5].value else {
            panic!("an arm set");
        };
        let arm = shape.nested(Site::of(&arms[0].1)).expect("the arm's block");
        let list = candidates(arm, &arm.body()[0]);
        assert_eq!(list.candidates, [local(1, registration)]);
    });
    let (_, message) = refusal(
        "MATCH 1 -> :Number WITH #{Number: ((EXPR #(HIDDEN x :Number) -> Number = #(x)) \
         (HIDDEN it))}\nLET a = (HIDDEN 1)",
    );
    assert_eq!(message, "`HIDDEN _` has no overload visible here");
}

#[test]
fn a_bucket_declaration_ranks_the_definitions_that_see_it() {
    let source = "EXPR #(MOVE 2 TO 1)\n\
                  EXPR #(MOVE x :Number TO y :Number) -> Number = #(x)\n\
                  LET a = (MOVE 1 TO 2)";
    built(source, |fixture, shape| {
        let [ranking] = shape.rankings() else {
            panic!("one declaration");
        };
        assert_eq!(ranking.classes, [1, 0]);
        assert_eq!(ranking.at, Position::statement(0));
        assert_eq!(ranking.key, fixture.symbols.key("MOVE _ TO _").unwrap());
        assert_eq!(shape.registrations()[0].classes, [1, 0]);
        let list = candidates(shape, node(&shape.body()[2].parts[3].value));
        assert_eq!(list.classes, [1, 0]);
        // A declaration binds nothing.
        assert_eq!(shape.slots(), 2);
    });
    // Two declarations of one ranking are one: `20 … 10` is `2 … 1`.
    built(
        "EXPR #(MOVE 2 TO 1)\nEXPR #(MOVE 20 TO 10)\n\
         EXPR #(MOVE x :Number TO y :Number) -> Number = #(x)",
        |_, shape| assert_eq!(shape.registrations()[0].classes, [1, 0]),
    );
    // With none visible, a definition ranks in written order.
    built(
        "EXPR #(MOVE x :Number TO y :Number) -> Number = #(x)\nEXPR #(MOVE _ TO _)",
        |_, shape| assert_eq!(shape.registrations()[0].classes, [0, 1]),
    );
}

#[test]
fn an_operator_registration_ranks_by_its_chaining() {
    built(
        "GROUP g FOLD RIGHT = (OP #(@) OVER Number = #(left))\n\
         GROUP h FOLD LEFT = (OP #(%) OVER Number = #(left))",
        |fixture, shape| {
            let right = birth(fixture, shape, "g");
            assert_eq!(right.registrations()[0].classes, [1, 0]);
            let left = birth(fixture, shape, "h");
            assert_eq!(left.registrations()[0].classes, [0, 1]);
        },
    );
}

#[test]
fn rankings_that_disagree_are_refused_where_they_meet() {
    for (source, at) in [
        // A declaration seeing a definition ranked another way.
        (
            "EXPR #(MOVE x :Number TO y :Number) -> Number = #(x)\nEXPR #(MOVE 2 TO 1)",
            "EXPR #(MOVE 2 TO 1)",
        ),
        // A declaration seeing a declaration.
        (
            "EXPR #(MOVE 2 TO 1)\nEXPR #(MOVE 1 TO 2)",
            "EXPR #(MOVE 1 TO 2)",
        ),
        // An inner declaration seeing an outer one.
        (
            "EXPR #(MOVE 2 TO 1)\nLET f = (FN :{} -> Null = #(EXPR #(MOVE _ TO _)))",
            "(EXPR #(MOVE _ TO _))",
        ),
        // A declaration at a key a builtin overload ranks in written order.
        ("EXPR #(ZZ 2 1)", "EXPR #(ZZ 2 1)"),
    ] {
        let (located, message) = refusal(source);
        let key = if source.contains("ZZ") {
            "ZZ _ _"
        } else {
            "MOVE _ TO _"
        };
        assert_eq!(
            message,
            format!("`{key}` is ranked two ways here"),
            "{source}"
        );
        assert_eq!(located, at, "{source}");
    }
}

#[test]
fn a_keyworded_definition_or_declaration_is_refused_where_it_cannot_register() {
    for (source, rendered) in [
        (
            "EXPR #(LET x :Number = y :Number) -> Number = #(x)",
            "`LET _ = _` is a builtin expression shape, which no definition adds to",
        ),
        (
            "EXPR #(EVAL 1 -> 2)",
            "`EVAL _ -> _` is a builtin expression shape, which no definition adds to",
        ),
        (
            "EXPR #(x :Number) -> Number = #(x)",
            "a definition's head must spell at least one keyword",
        ),
        (
            "EXPR #(MOVE 2 :Number TO 1 :Number) -> Number = #(1)",
            "a definition's head ranks no slot; declare the ranking on its own: `EXPR #(…)`",
        ),
        (
            "PRINT (LET doubled = 42)",
            "a binding must be its statement's own expression, not a part of one",
        ),
        (
            "LET a = (LET b = 1)",
            "a binding must be its statement's own expression, not a part of one",
        ),
        (
            "PRINT (EXPR #(MOVE 2 TO 1))",
            "a binding must be its statement's own expression, not a part of one",
        ),
    ] {
        assert_eq!(refusal(source).1, rendered, "{source}");
    }
    shaped("EXPR #(MOVE x TO 1)", |_, shape| {
        assert!(matches!(
            shape,
            Err(ShapeError::Malformed {
                form: BuiltinShapeId::BucketDeclaration,
                ..
            })
        ));
    });
}

#[test]
fn a_parenthesized_binder_is_its_statements_own() {
    built("(LET a = 1)\nLET b = a", |_, shape| {
        assert_eq!(shape.slots(), 2);
    });
}

#[test]
fn a_builtin_shape_dispatch_evaluates_selects_among_its_own_overloads() {
    built("LET v = 1\nLET a = (#[x] FROM v)", |_, shape| {
        let list = candidates(shape, node(&shape.body()[1].parts[3].value));
        assert!(
            list.candidates.is_empty(),
            "the suites' table holds no `FROM`"
        );
        assert_eq!(list.classes, [0, 1]);
    });
}

#[test]
fn a_keyworded_node_in_a_type_expression_is_no_use() {
    built("LET Widgets = :(WIDGET OF Number)", |_, shape| {
        let ExpressionPart::SigiledTypeExpr(written) = &shape.body()[0].parts[3].value else {
            panic!("a type expression");
        };
        assert!(
            shape
                .candidates(Site::of_node(written.reference()))
                .is_none()
        );
    });
}

#[test]
fn a_use_in_a_quotes_code_needs_no_candidate() {
    built("LET q = #(GREET 1)", |_, shape| {
        assert_eq!(shape.slots(), 1);
    });
}

#[test]
fn an_eager_cycle_through_a_registration_names_its_key() {
    let (_, message) = refusal("EXPR #(GREET x :Number) -> Number = #(a)\nLET a = (GREET 1)");
    assert_eq!(
        message,
        "these bindings need each other's values before any of them exists: `a` `GREET _`"
    );
}
