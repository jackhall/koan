//! Fixed shapes: the design document's classification examples, the diagnostics each error renders,
//! the kinds of nested shape, and the forms the builder refuses.

use crate::parse::BuiltinShapeId;
use crate::parse::{ExpressionPart, KExpression};
use crate::scope::{
    BodyShape, Builtins, CaptureSource, Coordinate, MentionClass, Position, ShapeError, ShapeKind,
    Site, Slot, Target, TopLevel,
};
use crate::symbols::BinderSymbol;

use super::{Fixture, NOWHERE, builtins, located, type_name, unlocated, value_name, with_fixture};

fn value(fixture: &Fixture<'_, '_>, text: &str) -> BinderSymbol {
    BinderSymbol::Value(value_name(text, fixture.symbols))
}

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
                fixture.symbols,
                fixture.scratch(),
            );
            check(fixture, &lines, shape)
        })
    })
}

/// The mention of `name` in `shape`, which must be unique.
fn mention_of<'graph>(
    shape: &BodyShape<'graph>,
    name: BinderSymbol,
) -> &'graph crate::scope::Mention {
    let mut found = shape
        .mentions()
        .iter()
        .filter(|mention| mention.name == name);
    let mention = found.next().expect("the name is mentioned");
    assert!(found.next().is_none(), "the name is mentioned once");
    mention
}

/// The shape nested at part `index` of a top-level line, reached through a parenthesized node.
fn nested_at<'graph>(
    shape: &BodyShape<'graph>,
    line: &KExpression<'graph>,
    index: usize,
    body: usize,
) -> &'graph BodyShape<'graph> {
    let ExpressionPart::Expression(node) = line.parts[index].value else {
        panic!("part {index} is a parenthesized node");
    };
    shape
        .nested(crate::scope::Site::of(&node.parts[body].value))
        .expect("the body has a shape")
}

#[test]
fn the_design_documents_classification_examples() {
    let source = "\
LET compute = (FN :{p :Number} -> Number = #(p))
LET f = (FN :{} -> Number = #(g))
LET x = [f (compute 5)]
LET y = (compute [f])
LET g = 1";
    shaped(source, |fixture, lines, shape| {
        let shape = shape.expect("the program shapes");
        let reads_of = |name| {
            let mut reads: Vec<_> = shape
                .mentions()
                .iter()
                .filter(|mention| mention.name == name)
                .map(|mention| (mention.at, mention.class))
                .collect();
            reads.sort_by_key(|(at, _)| *at);
            reads
        };
        let (f, compute) = (value(fixture, "f"), value(fixture, "compute"));
        // `x` holds `f` in a list and reads it at the end; `y` passes it to a call at its statement.
        assert_eq!(
            reads_of(f),
            [
                (Position::statement(3), MentionClass::Eager),
                (shape.end(), MentionClass::Deferred),
            ]
        );
        assert_eq!(
            reads_of(compute),
            [
                (Position::statement(2), MentionClass::Eager),
                (Position::statement(3), MentionClass::Eager),
            ]
        );

        // `f`'s body reads `g` at its own statement, but reaches it through a capture read at the
        // program's end, so the program-level edge is deferred and the component is `g` alone.
        let body = nested_at(shape, &lines[1], 3, 5);
        let g = value(fixture, "g");
        assert_eq!(mention_of(body, g).class, MentionClass::Eager);
        let (g_slot, _) = shape.slot(g).unwrap();
        assert_eq!(
            body.captures()[0].source,
            CaptureSource::Read(Coordinate::Activation {
                hops: 0,
                target: Target::Local(g_slot),
            })
        );
        assert_eq!(shape.component_of(g_slot).members, &[g_slot]);
    });
}

#[test]
fn a_root_mention_is_eager_and_a_forward_one_is_unbound() {
    shaped("LET a = b\nLET b = 1", |fixture, _, shape| {
        assert!(matches!(
            shape,
            Err(ShapeError::Unbound { name, at, .. })
                if name == value(fixture, "b") && located(at) == "b"
        ));
    });
}

#[test]
fn a_called_body_is_eager() {
    shaped(
        "LET z = ((FN :{} -> Number = #(z)) 1)",
        |fixture, _, shape| {
            assert!(matches!(
                shape,
                Err(ShapeError::Unbound { name, .. }) if name == value(fixture, "z")
            ));
        },
    );
}

#[test]
fn a_self_recursive_function_captures_itself_as_an_edge() {
    shaped(
        "LET f = (FN :{} -> Number = #(f))",
        |fixture, lines, shape| {
            let shape = shape.expect("the program shapes");
            let (slot, _) = shape.slot(value(fixture, "f")).unwrap();
            let component = shape.component_of(slot);
            assert!(component.deferred_only);
            assert_eq!(component.members, &[slot]);
            let body = nested_at(shape, &lines[0], 3, 5);
            assert_eq!(body.kind(), ShapeKind::Callable);
            assert_eq!(
                body.captures()[0].source,
                CaptureSource::Member {
                    component: shape.component_index(slot),
                    index: 0,
                }
            );
        },
    );
}

#[test]
fn a_ring_of_containers_is_one_deferred_component() {
    shaped("LET a = [b]\nLET b = [a]", |fixture, _, shape| {
        let shape = shape.expect("the program shapes");
        let (a, _) = shape.slot(value(fixture, "a")).unwrap();
        let (b, _) = shape.slot(value(fixture, "b")).unwrap();
        assert_eq!(shape.component_index(a), shape.component_index(b));
        assert!(shape.component_of(a).deferred_only);
    });
}

#[test]
fn a_cycle_through_a_call_is_an_eager_cycle() {
    shaped(
        "LET f = (FN :{} -> Number = #(x))\nLET x = (f 1)",
        |fixture, _, shape| {
            let Err(ShapeError::EagerCycle { members, .. }) = shape else {
                panic!("an eager cycle, got {:?}", shape.map(|_| ()));
            };
            let mut members = members.to_vec();
            members.sort();
            let mut expected = vec![value(fixture, "f"), value(fixture, "x")];
            expected.sort();
            assert_eq!(members, expected);
        },
    );
}

#[test]
fn a_function_recursive_with_one_inside_a_module_is_an_eager_cycle() {
    shaped(
        "LET f = (FN :{} -> Number = #(m))\nMODULE m = (LET g = (FN :{} -> Number = #(f)))",
        |_, _, shape| {
            assert!(matches!(shape, Err(ShapeError::EagerCycle { .. })));
        },
    );
}

#[test]
fn an_arm_binds_it_and_its_names_are_gone_after_it() {
    shaped(
        "LET q = 1\nMATCH q -> :Number WITH #{Number: ((LET w = it) (q))}",
        |fixture, lines, shape| {
            let shape = shape.expect("the program shapes");
            let ExpressionPart::DictLiteral(pairs) = lines[1].parts[5].value else {
                panic!("the arms are a dict");
            };
            let (_, arm_part) = &pairs[0];
            let arm = shape
                .nested(crate::scope::Site::of(arm_part))
                .expect("the arm has a shape");
            assert_eq!(arm.kind(), ShapeKind::Block);
            assert_eq!(arm.entered_at(), Position::statement(1));
            let (it, _) = arm.slot(value(fixture, "it")).unwrap();
            let (w, _) = arm.slot(value(fixture, "w")).unwrap();
            assert_eq!(
                mention_of(arm, value(fixture, "it")).coordinate,
                Coordinate::Activation {
                    hops: 0,
                    target: Target::Local(it),
                }
            );
            assert_ne!(it, w);
            let (q, _) = shape.slot(value(fixture, "q")).unwrap();
            assert_eq!(
                mention_of(arm, value(fixture, "q")).coordinate,
                Coordinate::Activation {
                    hops: 1,
                    target: Target::Local(q),
                }
            );
        },
    );
    shaped(
        "MATCH 1 -> :Number WITH #{Number: ((LET w = it) (w))}\nLET u = w",
        |fixture, _, shape| {
            assert!(matches!(
                shape,
                Err(ShapeError::Unbound { name, at, .. })
                    if name == value(fixture, "w") && located(at) == "w"
            ));
        },
    );
}

#[test]
fn a_union_may_name_itself_and_types_take_slots_after_values() {
    shaped(
        "UNION Nat = #{Zero: Null, Succ: Nat}\nLET one = 1",
        |fixture, _, shape| {
            let shape = shape.expect("the program shapes");
            let nat = BinderSymbol::Type(type_name("Nat", fixture.symbols));
            let (slot, position) = shape.slot(nat).unwrap();
            let values = (0..shape.slots())
                .filter(|index| {
                    matches!(shape.slot_name(Slot(*index as u32)), BinderSymbol::Value(_))
                })
                .count();
            assert_eq!(slot.index(), values);
            assert_eq!(position, Position::statement(0));
            assert_eq!(mention_of(shape, nat).class, MentionClass::Deferred);
            assert!(shape.component_of(slot).deferred_only);
            assert_eq!(shape.slots(), 2);
        },
    );
}

#[test]
fn each_error_names_what_a_user_needs() {
    let cases: &[(&str, &str)] = &[
        (
            "LET a = 1\nLET a = 2",
            "<input>:2:1: `a` is bound twice; first at <input>:1:1",
        ),
        (
            "LET origin = 1",
            "<input>:1:1: `origin` names a builtin, which cannot be rebound",
        ),
        (
            "LET a = b",
            "<input>:1:9: `b` names no binding visible here",
        ),
        (
            "LET f = (FN :{} -> Number = #(x))\nLET x = (f 1)",
            "<input>:1:1: these bindings need each other's values before any of them exists:",
        ),
        (
            "LET v = 1\nUSING v SCOPE (x)",
            "<input>:2:7: `USING` cannot tell which names this module surfaces;",
        ),
        (
            "MATCH 1 -> :Number WITH (Number (1))",
            "<input>:1:25: `Match` takes its arms as a dict of quotes: write #{…}",
        ),
        (
            "SIG Sg = #[(PRINT 1)]",
            "<input>:1:10: `Sig` takes :(LIST OF Declaration) as its part 3",
        ),
        (
            "UNION Empty = #{}",
            "<input>:1:15: `Union` takes :(MAP Name -> :(",
        ),
    ];
    for (source, message) in cases {
        shaped(source, |fixture, _, shape| {
            let error = shape.err().expect("the program is refused");
            let rendered = error.display(fixture.symbols, fixture.types).to_string();
            assert!(
                rendered.starts_with(message),
                "`{source}` rendered `{rendered}`"
            );
        });
    }
}

#[test]
fn each_error_points_at_what_it_is_about() {
    // Where the error `source` is refused with was found, and, for a rebind, its first binding.
    let refused = |source: &str| {
        shaped(source, |_, _, shape| {
            let error = shape.err().expect("the program is refused");
            let first = match error {
                ShapeError::Rebind { first, .. } => Some(located(first)),
                _ => None,
            };
            (located(error.at()), first)
        })
    };
    for (source, text) in [
        ("LET a = nowhere", "nowhere"),
        // A list's items carry no span, so an error about one points at the list.
        ("LET a = [1 nowhere]", "[1 nowhere]"),
        (
            "LET f = (FN :{} -> Number = #(x))\nLET x = (f 1)",
            "LET f = (FN :{} -> Number = #(x))",
        ),
        ("CLOSE (x)", "CLOSE (x)"),
        ("LET v = 1\nUSING v SCOPE (x)", "v"),
        ("LET d = {1: 2, _: 3}", "{1: 2, _: 3}"),
    ] {
        assert_eq!(refused(source).0, text, "{source}");
    }
    // A rebind names both declarations; a parameter is declared where its node is written.
    for (source, first, second) in [
        ("LET a = 1\nLET a = 2", "LET a = 1", "LET a = 2"),
        (
            "LET f = (FN :{x :Number} -> Number = #(LET x = 1))",
            "(FN :{x :Number} -> Number = #(LET x = 1))",
            "(LET x = 1)",
        ),
    ] {
        let (at_second, at_first) = refused(source);
        assert_eq!(at_first.as_deref(), Some(first), "{source}: a rebind");
        assert_eq!(at_second, second, "{source}");
    }
}

#[test]
fn every_capture_limiting_form_is_unsupported() {
    let cases: &[(&str, BuiltinShapeId)] = &[
        ("CLOSE OVER (x) (x)", BuiltinShapeId::CloseOver),
        ("CLOSE (x)", BuiltinShapeId::Close),
    ];
    for (source, form) in cases {
        shaped(source, |_, _, shape| {
            let error = shape.err().expect("the program is refused");
            assert_eq!(located(error.at()), *source, "the form node");
            assert_eq!(
                unlocated(error),
                ShapeError::Unsupported {
                    form: *form,
                    at: NOWHERE,
                }
            );
        });
    }
}

#[test]
fn a_signature_type_is_an_eager_mention_of_the_enclosing_shape() {
    shaped(
        "LET f = (FN :{n :Nat} -> Number = #(n))\nUNION Nat = #{Zero: Null}",
        |fixture, _, shape| {
            assert!(matches!(
                shape,
                Err(ShapeError::Unbound { name: BinderSymbol::Type(name), at, .. })
                    if name == type_name("Nat", fixture.symbols) && located(at) == "Nat"
            ));
        },
    );
    shaped(
        "EXPR FOR ALL #[Elt] #(HEAD xs :(LIST OF Elt)) -> Elt = #(xs)",
        |fixture, lines, shape| {
            let shape = shape.expect("a quantifier names nothing in the enclosing shape");
            let body = shape
                .nested(crate::scope::Site::of(&lines[0].parts[8].value))
                .expect("the body has a shape");
            let elt = BinderSymbol::Type(type_name("Elt", fixture.symbols));
            assert_eq!(
                body.slot(elt).map(|(_, position)| position),
                Some(Position::PARAMETER)
            );
            assert!(body.slot(value(fixture, "xs")).is_some());
        },
    );
}

#[test]
fn a_quantifier_read_in_its_body_is_a_mention_of_the_body_parameter() {
    shaped(
        "EXPR FOR ALL #[Elt] #(HEAD xs :(LIST OF Elt)) -> Elt = #(Elt)",
        |fixture, lines, shape| {
            let shape = shape.expect("the body reads its own type parameter");
            let body = shape
                .nested(crate::scope::Site::of(&lines[0].parts[8].value))
                .expect("the body has a shape");
            let elt = BinderSymbol::Type(type_name("Elt", fixture.symbols));
            let (slot, _) = body.slot(elt).expect("the quantifier is a body parameter");
            assert_eq!(
                mention_of(body, elt).coordinate,
                Coordinate::Activation {
                    hops: 0,
                    target: Target::Local(slot),
                }
            );
        },
    );
}

#[test]
fn a_binder_births_the_callable_at_its_root_and_the_body_knows_its_form() {
    let source = "\
LET f = (FN :{x :Number} -> Number = #(x))
LET wrapped = [(FN :{} -> Number = #(1))]
LET e = FN EXPR #(TWICE x :Number) -> Number = #(x)
LET plus = OP #(+) OVER Number = #(left)
LET k = 1";
    shaped(source, |fixture, lines, shape| {
        let shape = shape.expect("the program shapes");
        let slot = |name| shape.slot(value(fixture, name)).unwrap().0;
        let f = shape.births(slot("f")).expect("a root FN is a birth");
        assert_eq!(f.kind(), ShapeKind::Callable);
        assert_eq!(
            f.form()
                .and_then(|form| form.cache().builtin_shape())
                .map(|form| form.id),
            Some(BuiltinShapeId::Lambda)
        );
        assert!(std::ptr::eq(f, nested_at(shape, &lines[0], 3, 5)));
        let e = shape.births(slot("e")).expect("a combined EXPR is a birth");
        assert_eq!(
            e.form()
                .and_then(|form| form.cache().builtin_shape())
                .map(|form| form.id),
            Some(BuiltinShapeId::CombinedExpression)
        );
        let plus = shape
            .births(slot("plus"))
            .expect("a combined OP is a birth");
        assert_eq!(
            plus.form()
                .and_then(|form| form.cache().builtin_shape())
                .map(|form| form.id),
            Some(BuiltinShapeId::CombinedOperator)
        );
        assert!(
            shape.births(slot("wrapped")).is_none(),
            "a FN in a list is not at the root"
        );
        assert!(shape.births(slot("k")).is_none());
        assert!(shape.form().is_none(), "the program sits in no form");
    });
}

#[test]
fn a_nominal_construction_reads_its_head_eagerly_and_its_payload_as_a_constructor_slot() {
    shaped(
        "LET a = (Ring {next = a})\nLET k = [1]",
        |fixture, _, shape| {
            let shape = shape.expect("a tagged self-reference shapes");
            let a = value(fixture, "a");
            assert_eq!(mention_of(shape, a).class, MentionClass::Deferred);
            let ring = BinderSymbol::Type(type_name("Ring", fixture.symbols));
            assert_eq!(mention_of(shape, ring).class, MentionClass::Eager);
            let (slot, _) = shape.slot(a).unwrap();
            let component = shape.component_of(slot);
            assert_eq!(component.members, &[slot]);
            assert!(component.deferred_only && component.cyclic);
            let (k, _) = shape.slot(value(fixture, "k")).unwrap();
            assert!(
                !shape.component_of(k).cyclic,
                "a data binder reading nothing of its own"
            );
        },
    );
    shaped("LET b = [a]\nLET a = (Ring {next = b})", |_, _, shape| {
        let shape = shape.expect("a tagged ring through an earlier binder shapes");
        assert!(
            shape
                .components()
                .iter()
                .any(|component| component.members.len() == 2)
        );
    });
    shaped(
        "LET f = 1\nLET b = [a]\nLET a = (f {next = b})",
        |fixture, _, shape| {
            let Err(ShapeError::EagerCycle { members, .. }) = shape else {
                panic!("a call headed by a name reads every part eagerly");
            };
            let mut members = members.to_vec();
            members.sort();
            let mut expected = vec![value(fixture, "a"), value(fixture, "b")];
            expected.sort();
            assert_eq!(members, expected);
        },
    );
}

#[test]
fn a_let_binder_records_its_right_hand_side() {
    shaped(
        "LET a = [1 2]\nNEWTYPE Distance = Number",
        |fixture, lines, shape| {
            let shape = shape.expect("the program shapes");
            let (a, _) = shape.slot(value(fixture, "a")).unwrap();
            let rhs = shape.rhs(a).expect("a LET has a right-hand side");
            assert!(std::ptr::eq(rhs, &lines[0].parts[3].value));
            let distance = BinderSymbol::Type(type_name("Distance", fixture.symbols));
            let (declared, _) = shape.slot(distance).unwrap();
            assert!(shape.rhs(declared).is_none(), "a NEWTYPE has none");
        },
    );
}

#[test]
fn a_type_binder_records_its_declaration_node() {
    shaped(
        "LET a = [1 2]\nNEWTYPE Distance = Number",
        |fixture, lines, shape| {
            let shape = shape.expect("the program shapes");
            let distance = BinderSymbol::Type(type_name("Distance", fixture.symbols));
            let (declared, _) = shape.slot(distance).unwrap();
            let node = shape
                .declarations(declared)
                .expect("a type binder records its declaration");
            assert_eq!(
                node.cache().builtin_shape().map(|form| form.id),
                Some(BuiltinShapeId::NewTypeDefinition)
            );
            assert!(std::ptr::eq(node.parts, lines[1].parts));
            let (a, _) = shape.slot(value(fixture, "a")).unwrap();
            assert!(
                shape.declarations(a).is_none(),
                "a value binder records none"
            );
        },
    );
}

#[test]
fn a_signature_body_declares_its_own_members() {
    // A `SIG`'s head parameters, its members' `FOR ALL` names and its manifest `LET` members are
    // the definition's own: none is a mention of the enclosing shape.
    for (source, own) in [
        (
            "SIG Headed FOR ALL #{Key: Any, Val: Any} = #[(VAL k :Key) (VAL v :Val)]",
            &["Key", "Val"][..],
        ),
        (
            "SIG Boxy FOR ALL #[Elem] = \
             #[(VAL unbox :(EXPR FOR ALL #[Held] #(TAKE it :Held) -> Elem))]",
            &["Elem", "Held"],
        ),
        (
            "SIG Fixed = #[(LET Elem = Number) (VAL x :Elem)]",
            &["Elem"],
        ),
    ] {
        shaped(source, |fixture, _, shape| {
            let shape = shape.unwrap_or_else(|error| {
                panic!(
                    "`{source}` shapes: {}",
                    error.display(fixture.symbols, fixture.types)
                )
            });
            assert_eq!(shape.slots(), 1, "`{source}` declares the signature alone");
            for name in own {
                let name = BinderSymbol::Type(type_name(name, fixture.symbols));
                assert!(
                    !shape.mentions().iter().any(|mention| mention.name == name),
                    "`{source}` declares `{name:?}` in its definition"
                );
            }
        });
    }
}

#[test]
fn a_type_member_is_refused_where_it_is_written() {
    // A signature hides a type through a head parameter, so `TYPE` is a reserved shape.
    shaped(
        "SIG Shown = #[(TYPE Carrier) (VAL zero :Carrier)]",
        |_, _, shape| {
            assert!(matches!(
                shape.err(),
                Some(ShapeError::Unsupported {
                    form: BuiltinShapeId::TypeDeclaration,
                    ..
                })
            ));
        },
    );
}

#[test]
fn a_parameterized_union_declares_its_parameters() {
    // A parameterized union's declarator declares the names its variants' payloads read: none is a
    // mention of the enclosing shape, while any other type a payload names still is.
    for (source, own) in [
        (
            "UNION (Elem AS Option) = #{Some: Elem, None: Null}",
            &["Elem"][..],
        ),
        (
            "UNION (Ok Error AS Result) = #{Ok: Ok, Error: Error}",
            &["Ok", "Error"],
        ),
    ] {
        shaped(source, |fixture, _, shape| {
            let shape = shape.unwrap_or_else(|error| {
                panic!(
                    "`{source}` shapes: {}",
                    error.display(fixture.symbols, fixture.types)
                )
            });
            assert_eq!(shape.slots(), 1, "`{source}` declares the union alone");
            for name in own {
                let name = BinderSymbol::Type(type_name(name, fixture.symbols));
                assert!(
                    !shape.mentions().iter().any(|mention| mention.name == name),
                    "`{source}` declares `{name:?}` in its declarator"
                );
            }
        });
    }
    shaped(
        "UNION (Elem AS Option) = #{Some: Elem, None: Null}",
        |fixture, _, shape| {
            let shape = shape.expect("the program shapes");
            mention_of(
                shape,
                BinderSymbol::Type(type_name("Null", fixture.symbols)),
            );
        },
    );
}

#[test]
fn a_signature_body_reads_the_types_it_does_not_declare() {
    shaped(
        "NEWTYPE Distance = Number\nSIG Far = #[(VAL how_far :Distance)]",
        |fixture, _, shape| {
            let shape = shape.expect("the program shapes");
            let distance = BinderSymbol::Type(type_name("Distance", fixture.symbols));
            let mention = mention_of(shape, distance);
            assert_eq!(mention.class, MentionClass::Deferred);
        },
    );
}

#[test]
fn a_pin_reads_its_type_and_labels_its_parameter() {
    shaped(
        "NEWTYPE Distance = Number\n\
         SIG Stack FOR ALL #[Elt] = #[(VAL top :Elt)]\n\
         LET Far = :(Stack WITH {Elt = Distance})",
        |fixture, _, shape| {
            let shape = shape.expect("the program shapes");
            let name = |text| BinderSymbol::Type(type_name(text, fixture.symbols));
            mention_of(shape, name("Stack"));
            mention_of(shape, name("Distance"));
            assert!(
                !shape
                    .mentions()
                    .iter()
                    .any(|mention| mention.name == name("Elt")),
                "a pin's key names a parameter, not a type in scope"
            );
        },
    );
}

/// The one block shape in `shape`'s tree — each program below holds a single `USING` body.
fn only_block<'graph>(shape: &BodyShape<'graph>) -> &'graph BodyShape<'graph> {
    fn collect<'graph>(shape: &BodyShape<'graph>, found: &mut Vec<&'graph BodyShape<'graph>>) {
        for (_, child) in shape.nested_shapes() {
            if child.kind() == ShapeKind::Block {
                found.push(child);
            }
            collect(child, found);
        }
    }
    let mut found = Vec::new();
    collect(shape, &mut found);
    assert_eq!(found.len(), 1, "the program holds one block");
    found[0]
}

/// The names `block` declares, spelled out.
fn parameters_of(fixture: &Fixture<'_, '_>, block: &BodyShape<'_>) -> Vec<String> {
    (0..block.slots())
        .map(|slot| block.slot_name(Slot(slot as u32)))
        .filter(|name| !matches!(name, BinderSymbol::Registration(_)))
        .map(|name| fixture.symbols.display(name.symbol()).to_string())
        .collect()
}

#[test]
fn a_module_binder_births_a_module_body() {
    let source = "MODULE m = ((LET x = 1) (NEWTYPE Dist = Number))";
    shaped(source, |fixture, _, shape| {
        let shape = shape.expect("the program shapes");
        let (slot, _) = shape.slot(value(fixture, "m")).expect("`m` is bound");
        let body = shape.births(slot).expect("a module binder births its body");
        assert_eq!(body.kind(), ShapeKind::Module);
        assert!(body.form().is_none(), "a module body has no callable form");
        let component = &shape.components()[shape.component_index(slot).index()];
        assert_eq!(component.members, &[slot]);
        assert!(!component.cyclic, "a module is alone in its component");
    });
}

#[test]
fn a_module_naming_itself_from_a_body_of_its_own_is_refused() {
    // A function outside a module mutually recursive with one inside it is the eager cycle pinned
    // above. A module naming *itself* is refused a step earlier: every mention reached from a
    // module binder's root is eager whatever body it sits in, and an eager read at the binder's own
    // position does not see that binder. So a module is never in a cycle with itself, and every
    // module born is a one-node knot.
    shaped(
        "MODULE m = (LET f = (FN :{} -> Number = #(m)))",
        |_, _, shape| {
            assert!(matches!(shape.err(), Some(ShapeError::Unbound { .. })));
        },
    );
}

#[test]
fn a_using_body_takes_its_operands_surfaced_names_as_parameters() {
    let module = "MODULE m = ((LET x = 1) (NEWTYPE Dist = Number))";
    let shown = "SIG Shown FOR ALL #[Carrier] = #[(VAL zero :Carrier)]";
    let cases: &[(String, &[&str])] = &[
        // A `MODULE` binder read directly, and the same read from a body that precedes it.
        (format!("{module}\nUSING m SCOPE (x)"), &["x", "Dist"]),
        // A `GROUP` binder births the same body, so it reads the same way.
        (
            "GROUP g FOLD LEFT = ((LET x = 1) (NEWTYPE Dist = Number) (OP #(@) OVER Number = #(left)))\nUSING g SCOPE (x)"
                .to_string(),
            &["x", "Dist"],
        ),
        (
            "LET f = (FN :{} -> Number = #(USING m SCOPE (x)))\nMODULE m = (LET x = 1)"
                .to_string(),
            &["x"],
        ),
        // The `SIG` an ascription at the site names, opaque and transparent.
        (
            format!("{module}\n{shown}\nUSING (m :| Shown) SCOPE (zero)"),
            &["zero", "Carrier"],
        ),
        // A `LET` rooted at an ascription, and a `LET` rooted at that `LET`.
        (
            format!("{module}\n{shown}\nLET v = (m :! Shown)\nUSING v SCOPE (zero)"),
            &["zero", "Carrier"],
        ),
        (
            format!("{module}\n{shown}\nLET v = (m :! Shown)\nLET w = v\nUSING w SCOPE (zero)"),
            &["zero", "Carrier"],
        ),
        // A type alias, and a `WITH` pin — neither changes a name.
        (
            format!("{module}\n{shown}\nLET Alias = Shown\nUSING (m :| Alias) SCOPE (zero)"),
            &["zero", "Carrier"],
        ),
        (
            format!(
                "{module}\n{shown}\nUSING (m :| (Shown WITH {{Carrier = Number}})) SCOPE (zero)"
            ),
            &["zero", "Carrier"],
        ),
    ];
    for (source, expected) in cases {
        shaped(source, |fixture, _, shape| {
            let shape = shape.unwrap_or_else(|error| {
                panic!(
                    "`{source}` shapes: {}",
                    error.display(fixture.symbols, fixture.types)
                )
            });
            let mut names = parameters_of(fixture, only_block(shape));
            let mut want: Vec<String> = expected.iter().map(|name| name.to_string()).collect();
            names.sort();
            want.sort();
            assert_eq!(names, want, "`{source}` surfaces the wrong names");
        });
    }
}

#[test]
fn a_surfaced_name_resolves_like_any_other_local() {
    let source = "MODULE m = ((LET x = 1) (NEWTYPE Dist = Number))\nUSING m SCOPE (x)";
    shaped(source, |fixture, _, shape| {
        let shape = shape.expect("the program shapes");
        let block = only_block(shape);
        let mention = mention_of(block, value(fixture, "x"));
        let (slot, _) = block.slot(value(fixture, "x")).expect("`x` is a parameter");
        assert_eq!(
            mention.coordinate,
            Coordinate::Activation {
                hops: 0,
                target: Target::Local(slot),
            },
        );
        assert_eq!(mention.class, MentionClass::Eager);
    });
}

#[test]
fn a_callable_in_a_using_body_captures_a_surfaced_name_by_reading_it() {
    let source = "MODULE m = (LET x = 1)\nUSING m SCOPE (LET f = (FN :{} -> Number = #(x)))";
    shaped(source, |fixture, _, shape| {
        let shape = shape.expect("the program shapes");
        let block = only_block(shape);
        let (slot, _) = block.slot(value(fixture, "x")).expect("`x` is a parameter");
        let (_, callable) = block.nested_shapes()[0];
        let capture = &callable.captures()[0];
        assert_eq!(capture.name, value(fixture, "x"));
        assert_eq!(
            capture.source,
            CaptureSource::Read(Coordinate::Activation {
                hops: 0,
                target: Target::Local(slot),
            }),
        );
    });
}

#[test]
fn an_operand_whose_names_cannot_be_read_is_refused() {
    let module = "MODULE m = (LET x = 1)";
    let shown = "SIG Shown FOR ALL #[Carrier] = #[(VAL zero :Carrier)]";
    let cases = [
        // A parameter typed by a signature may hold a wider module, so it is never readable.
        format!("{shown}\nLET f = (FN :{{m :Shown}} -> Number = #(USING m SCOPE (zero)))"),
        // A call says nothing statically.
        format!("{module}\nLET f = (FN :{{}} -> Number = #(1))\nUSING (f {{}}) SCOPE (x)"),
        // A member read is not a module the reader can follow.
        format!("{module}\nUSING m.x SCOPE (x)"),
    ];
    for source in &cases {
        shaped(source, |_, _, shape| {
            let error = shape.err();
            assert!(
                matches!(error, Some(ShapeError::Unsurfaced { .. })),
                "`{source}` is refused as unsurfaced, not {error:?}",
            );
        });
    }
}

#[test]
fn a_bound_is_a_mention_of_the_enclosing_shape() {
    let missing = |fixture: &Fixture<'_, '_>, shape: Result<&BodyShape<'_>, ShapeError>| {
        assert!(matches!(
            shape,
            Err(ShapeError::Unbound { name: BinderSymbol::Type(name), .. })
                if name == type_name("Missing", fixture.symbols)
        ));
    };
    // A bound naming nothing in scope is unbound, in a callable's `FOR ALL` group and a
    // signature's.
    shaped(
        "LET f = (FN FOR ALL #{Elt: Missing} :{x :Elt y :Elt} -> Elt = #(x))",
        |fixture, _, shape| missing(fixture, shape),
    );
    shaped(
        "SIG Shown FOR ALL #{Carrier: Missing} = #[(VAL zero :Carrier)]",
        |fixture, _, shape| missing(fixture, shape),
    );
    // A bound naming a declared type is that type's mention; the bounded name is the body's.
    shaped(
        "NEWTYPE Dist = Number\nLET f = (FN FOR ALL #{Elt: Dist, Key: Any} :{x :Elt y :Key} -> Elt = #(x))",
        |fixture, lines, shape| {
            let shape = shape.expect("the program shapes");
            let dist = BinderSymbol::Type(type_name("Dist", fixture.symbols));
            let _ = mention_of(shape, dist);
            let body = nested_at(shape, &lines[1], 3, 8);
            for name in ["Elt", "Key"] {
                let name = BinderSymbol::Type(type_name(name, fixture.symbols));
                assert_eq!(
                    body.slot(name).map(|(_, position)| position),
                    Some(Position::PARAMETER)
                );
            }
            assert!(body.slot(dist).is_none());
        },
    );
    // A bounded head parameter is the signature's own, and its bound a mention.
    shaped(
        "SIG Shown FOR ALL #{Carrier: Number} = #[(VAL zero :Carrier)]",
        |fixture, _, shape| {
            let shape = shape.expect("the program shapes");
            let carrier = BinderSymbol::Type(type_name("Carrier", fixture.symbols));
            assert!(
                !shape
                    .mentions()
                    .iter()
                    .any(|mention| mention.name == carrier)
            );
            // The head group is read where the declaration is, as a callable's group is.
            let number = BinderSymbol::Type(type_name("Number", fixture.symbols));
            assert_eq!(mention_of(shape, number).class, MentionClass::Eager);
        },
    );
}

#[test]
fn a_lambdas_body_is_found_by_its_forms_body_site() {
    shaped("LET k = 1\n(FN :{} -> Number = #(k))", |_, _, shape| {
        let shape = shape.expect("the program shapes");
        let node = shape.body()[1].statement_spine();
        let body = shape
            .nested(Site::of_body(node).expect("a `FN` has a body"))
            .expect("the body has a shape");
        assert_eq!(body.kind(), ShapeKind::Callable);
        // The shape keeps a resident copy of the node, over the same parts.
        let form = body.form().expect("a `FN` records its form");
        assert!(std::ptr::eq(form.parts, node.parts));
        assert_eq!(Site::of_body(&shape.body()[0]), None);
    });
}

const PICK: &str = "LET pick = (FN FOR ALL #[Elt] :{x :Elt} -> Elt = #(x))";

/// A keyworded form's name and a surfaced quantified member are read only at the head of a call; a
/// module body's quantified member is read unmarked anywhere, the static pass instantiating it, and
/// any other quantified binding is the static pass's to instantiate or refuse.
#[test]
fn a_call_only_quantified_function_is_read_only_at_the_head_of_a_call() {
    let module = "MODULE m = (LET pick = (FN FOR ALL #[Elt] :{x :Elt} -> Elt = #(x)))";
    let identity = "SIG Ident = #[(VAL identity :(FN FOR ALL #[Item] :{x :Item} -> Item))]";
    let member = |rest: &str| format!("MODULE lib = (({PICK}) ({rest}))");
    for (source, name) in [
        (
            "LET pick = FN EXPR FOR ALL #[Elt] #(PICK x :Elt) -> Elt = #(x)\nLET keep = [pick]"
                .to_string(),
            "pick",
        ),
        (format!("{module}\nUSING m SCOPE (pick)"), "pick"),
        (
            format!("{module}\n{identity}\nUSING (m :! Ident) SCOPE (identity)"),
            "identity",
        ),
        // A `$` name reads where the quote is written, and is never an instance site.
        (member("LET q = #($pick)"), "pick"),
        (
            member("LET q = #(LET f = (FN :{} -> Any = #([$pick])))"),
            "pick",
        ),
        // An `EVAL` offering the name passes its value into the code.
        (
            member(
                "LET run = (FN :{body :(Expression NEEDING #[pick])} -> Any = \
                 #(EVAL body -> Any))",
            ),
            "pick",
        ),
    ] {
        shaped(&source, |_, _, shape| {
            let name = BinderSymbol::classify(name).unwrap();
            assert!(
                matches!(
                    shape.err(),
                    Some(ShapeError::QuantifiedRead { name: read, .. }) if read == name
                ),
                "`{source}` refuses the read"
            );
        });
    }
    for source in [
        format!("{PICK}\nLET keep = [(FN :{{x :Number}} -> Number = #(pick {{x = x}}))]"),
        format!("{PICK}\nPRINT (pick {{x = 1}})"),
        format!("{PICK}\nPRINT ((pick) {{x = 1}})"),
        format!("{PICK}\nLET keep = [pick]"),
        member("LET keep = [pick]"),
        member("LET f = (FN :{} -> Any = #([pick]))"),
        format!("{module}\nUSING m SCOPE (pick {{x = 1}})"),
        format!("{module}\n{identity}\nUSING (m :! Ident) SCOPE (identity {{x = 1}})"),
        // A quantified function calling itself by name.
        "LET loop = (FN FOR ALL #[Elt] :{x :Elt} -> Elt = #(loop {x = x}))".to_string(),
        member("LET q = #($pick {x = 1})"),
        "LET keep = [(FN FOR ALL #[Elt] :{x :Elt} -> Elt = #(x))]".to_string(),
        "PRINT ((FN FOR ALL #[Elt] :{x :Elt} -> Elt = #(x)) {x = 1})".to_string(),
    ] {
        shaped(&source, |fixture, _, shape| {
            if let Err(error) = shape {
                panic!(
                    "`{source}` shapes: {}",
                    error.display(fixture.symbols, fixture.types)
                );
            }
        });
    }
}

/// A body whose value is read takes its last statement's, so one binding a quantified function by a
/// keyworded form there would hand on a value read only at a call's head; a plain `LET` there is
/// the static pass's to instantiate at the body's return.
#[test]
fn a_body_s_value_binds_no_keyworded_quantified_function() {
    let expression = "EXPR FOR ALL #[Elt] #(ID x :Elt) -> Elt = #(x)";
    let combined = "LET id = FN EXPR FOR ALL #[Elt] #(ID x :Elt) -> Elt = #(x)";
    for source in [
        format!("LET f = (FN :{{}} -> Any = #({expression}))"),
        format!("LET f = (FN :{{}} -> Any = #({combined}))"),
        format!("LET f = (FN :{{}} -> Any = #(\n  LET y = 1\n  {expression}\n))"),
    ] {
        shaped(&source, |_, _, shape| {
            assert!(
                matches!(shape.err(), Some(ShapeError::QuantifiedValue { .. })),
                "`{source}` refuses the body's value"
            );
        });
    }
    for source in [
        PICK.to_string(),
        format!("LET f = (FN :{{}} -> Number = #(\n  {PICK}\n  pick {{x = 1}}\n))"),
        format!("LET f = (FN :{{}} -> Any = #({PICK}))"),
        format!("MODULE m = ({PICK})"),
    ] {
        shaped(&source, |fixture, _, shape| {
            if let Err(error) = shape {
                panic!(
                    "`{source}` shapes: {}",
                    error.display(fixture.symbols, fixture.types)
                );
            }
        });
    }
}

/// An `OVER` entry the body never reads is captured all the same, and a top-level one counts by
/// its binding, which the run reads where it lives.
#[test]
fn an_over_entry_is_part_of_the_module_whether_or_not_it_is_read() {
    let source = "LET top = 1\n\
                  EXPR #(MK unread :Number) -> Any = #(MODULE m OVER #[unread top] = (LET x = 2))";
    shaped(source, |fixture, _, shape| {
        let shape = shape.unwrap_or_else(|error| {
            panic!(
                "the program shapes: {}",
                error.display(fixture.symbols, fixture.types)
            )
        });
        let (_, callable) = shape.nested_shapes()[0];
        let (_, module) = callable.nested_shapes()[0];
        assert_eq!(module.kind(), ShapeKind::Module);
        let captured: Vec<BinderSymbol> = module.captures().iter().map(|c| c.name).collect();
        assert_eq!(
            captured,
            vec![value(fixture, "unread")],
            "only the outer entry is a capture"
        );
        let (top, _) = shape.slot(value(fixture, "top")).expect("`top` is bound");
        assert_eq!(module.listed_top(), &[TopLevel::Root(top)]);
    });
}

/// A body's code digest is its code as resolved: blind to where the body sits, and naming a read of
/// the top level by the binding it reads.
#[test]
fn a_code_digest_is_blind_to_position_and_names_top_level_reads_by_binding() {
    let source = "LET t = 1\nLET u = 1\n\
                  LET f = (FN :{x :Any} -> Any = #(t))\n\
                  LET pad = 2\n\
                  LET g = (FN :{x :Any} -> Any = #(t))\n\
                  LET h = (FN :{x :Any} -> Any = #(u))";
    shaped(source, |fixture, _, shape| {
        let shape = shape.expect("the program shapes");
        let code = |name: &str| {
            let (slot, _) = shape.slot(value(fixture, name)).expect("a binder");
            shape.births(slot).expect("a callable").code_digest()
        };
        assert_eq!(code("f"), code("g"), "one text at two places");
        assert_ne!(code("f"), code("h"), "two top-level bindings of one value");
    });
}
