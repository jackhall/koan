//! Each production a type expression elaborates, each refusal, and a callable's type read off each
//! form that births one.

use crate::memory::BumpAllocator;
use crate::parse::{ExpressionPart, KExpression};
use crate::scope::{BodyShape, Position, Site, Slot};
use crate::symbols::{BinderSymbol, KeywordSymbol, SymbolInterner};
use crate::type_lattice::{
    DispatchTokenElement, KType, RecursiveGroupWindow, RelativeSchema, TypeRegistry,
};
use crate::values::Value;

use super::super::{Canonical, Elaboration, callable_type, type_expression};
use super::{Held, Program, nulls, scalars, with_program};

/// The right-hand side of `LET <name> = <rhs>` on `line`.
fn rhs<'graph>(line: &KExpression<'graph>) -> &'graph ExpressionPart<'graph> {
    let spine = line.statement_spine();
    &spine.parts[3].value
}

fn elaborated(program: &Program<'_, '_, '_>, line: usize) -> Result<KType, Elaboration> {
    type_expression(
        rhs(&program.lines[line]),
        program.activation,
        program.types,
        program.scratch,
    )
}

fn keyword(text: &str, symbols: &SymbolInterner) -> DispatchTokenElement {
    DispatchTokenElement::Keyword(KeywordSymbol::declared(text, symbols).expect("a keyword"))
}

#[test]
fn each_composite_production_builds_its_lattice_node() {
    let source = "\
LET Alias = Str
LET Listed = :(LIST OF Alias)
LET Mapped = :(MAP Str -> (LIST OF Number))
LET Either = :(Number | Str | Null)
LET Fields = :{x :Number, y :Alias}
LET Function = :(FN :{x :Number} -> Bool)
LET Headed = :(EXPR (TWICE x :Number) -> Number)
LET Bare = Alias";
    with_program(
        source,
        scalars,
        |name, writer, types| match name {
            "Alias" => Held::Bound(Value::Type(crate::values::TypeValue::new(
                writer,
                KType::STR,
                types,
            ))),
            _ => Held::Bound(Value::Null),
        },
        |program| {
            let (types, scratch) = (program.types, program.scratch);
            let x = BinderSymbol::classify("x").unwrap();
            let y = BinderSymbol::classify("y").unwrap();
            assert_eq!(elaborated(&program, 1), Ok(types.list(KType::STR)));
            assert_eq!(
                elaborated(&program, 2),
                Ok(types.dict(KType::STR, types.list(KType::NUMBER)))
            );
            assert_eq!(
                elaborated(&program, 3),
                Ok(types.union_of(scratch, &[KType::NUMBER, KType::STR, KType::NULL]))
            );
            assert_eq!(
                elaborated(&program, 4),
                Ok(types.record(scratch, &[(x, KType::NUMBER), (y, KType::STR)]))
            );
            assert_eq!(
                elaborated(&program, 5),
                Ok(types
                    .function_type(scratch, &[], &[(x, KType::NUMBER)], KType::BOOL)
                    .handle)
            );
            let twice = [
                keyword("TWICE", program.symbols),
                DispatchTokenElement::Slot(KType::NUMBER),
            ];
            assert_eq!(
                elaborated(&program, 6),
                Ok(types.shape_type(scratch, &[], &twice, KType::NUMBER).handle)
            );
            assert_eq!(elaborated(&program, 7), Ok(KType::STR));
        },
    );
}

#[test]
fn a_quantified_head_interns_by_shape_whatever_its_names() {
    let source = "\
LET Named = :(EXPR FOR ALL (Elt) (ID x :Elt) -> Elt)
LET Renamed = :(EXPR FOR ALL (Other) (ID x :Other) -> Other)
LET Fixed = :(EXPR (ID x :Number) -> Number)";
    with_program(source, scalars, nulls, |program| {
        let named = elaborated(&program, 0).unwrap();
        assert_eq!(elaborated(&program, 1), Ok(named));
        assert_ne!(elaborated(&program, 2), Ok(named));
    });
}

#[test]
fn a_quantified_lambda_type_interns_by_shape_whatever_its_names() {
    let source = "\
LET Named = :(FN FOR ALL (Elt) :{x :Elt} -> Elt)
LET Renamed = :(FN FOR ALL (Other) :{x :Other} -> Other)
LET Fixed = :(FN :{x :Number} -> Number)";
    with_program(source, scalars, nulls, |program| {
        let named = elaborated(&program, 0).unwrap();
        assert_eq!(elaborated(&program, 1), Ok(named));
        assert_ne!(elaborated(&program, 2), Ok(named));
    });
}

#[test]
fn a_function_type_inside_a_quantified_head_reads_the_heads_variable() {
    // A bare `FN` type opens no group, so its field and return keep reading the head's `Elt`.
    let source =
        "LET Applied = :(EXPR FOR ALL (Elt) (APPLY f :(FN :{x :Elt} -> Elt) TO v :Elt) -> Elt)";
    with_program(source, scalars, nulls, |program| {
        let (types, scratch) = (program.types, program.scratch);
        let quantified = types.quantified(0, KType::ANY);
        let x = BinderSymbol::classify("x").unwrap();
        let inner = types
            .function_type(scratch, &[], &[(x, quantified)], quantified)
            .handle;
        let elt = program.type_name("Elt");
        assert_eq!(
            elaborated(&program, 0),
            Ok(types
                .shape_type(
                    scratch,
                    &[elt],
                    &[
                        keyword("APPLY", program.symbols),
                        DispatchTokenElement::Slot(inner),
                        keyword("TO", program.symbols),
                        DispatchTokenElement::Slot(quantified),
                    ],
                    quantified
                )
                .handle)
        );
    });
}

#[test]
fn an_outer_quantifier_read_under_a_nested_function_group_is_refused() {
    // The nested `FN FOR ALL` opens a group of its own, which shadows the head's `Elt`.
    let source = "LET Shadowed = :(EXPR FOR ALL (Elt) \
                    (APPLY f :(FN FOR ALL (Other) :{x :Elt} -> Other) TO v :Elt) -> Elt)";
    with_program(source, scalars, nulls, |program| {
        assert!(matches!(
            elaborated(&program, 0),
            Err(Elaboration::Unsupported { .. })
        ));
    });
}

/// A union `Shape` of two singleton members, `Circle` and `Square`.
fn shapes(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    symbols: &SymbolInterner,
) -> Vec<(&'static str, KType)> {
    let member = |name| {
        RecursiveGroupWindow::seal_singleton(
            scratch,
            crate::symbols::TypeSymbol::declared(name, symbols).unwrap(),
            RelativeSchema::NewType(KType::NUMBER),
            None,
            types,
            scratch,
        )
    };
    let union = types.union_of(scratch, &[member("Circle"), member("Square")]);
    vec![("Shape", union), ("Wrap", KType::ANY)]
}

#[test]
fn a_union_member_projects_by_its_tag() {
    let source = "\
LET Round = :(Shape.Circle)
LET Missing = :(Shape.Triangle)";
    with_program(source, shapes, nulls, |program| {
        let member = elaborated(&program, 0).expect("`Shape.Circle` elaborates");
        assert!(matches!(
            program.types.node(member),
            crate::type_lattice::TypeNode::SetMember { name, .. } if name == program.type_name("Circle")
        ));
        assert!(matches!(
            elaborated(&program, 1),
            Err(Elaboration::NoSuchMember { name, .. }) if name == program.type_name("Triangle").symbol()
        ));
    });
}

#[test]
fn a_name_reads_not_a_type_and_application_is_unsupported() {
    let source = "\
LET Data = Str
LET Wrong = :(LIST OF Data)
LET Applied = :(Number AS Wrap)";
    with_program(
        source,
        shapes,
        |_, _, _| Held::Bound(Value::Number(1.0)),
        |program| {
            assert!(matches!(
                elaborated(&program, 1),
                Err(Elaboration::NotAType { name, .. }) if name == program.type_name("Data")
            ));
            assert_eq!(
                elaborated(&program, 2),
                Err(Elaboration::Unsupported {
                    site: Site::of(rhs(&program.lines[2])),
                })
            );
        },
    );
}

#[test]
fn a_callable_type_is_read_off_the_form_that_births_it() {
    let source = "\
LET f = (FN :{x :Number, ys :(LIST OF Str)} -> Bool = (x))
LET twice = FN EXPR (TWICE x :Number) -> Number = (x)
LET id = FN EXPR FOR ALL (Elt) (ID x :Elt) -> Elt = (x)
LET lambda_id = (FN FOR ALL (Elt) :{x :Elt} -> Elt = (x))
LET plus = OP #(+) OVER Number = (left)
LET less = OP #(<) OVER Number -> Bool = (left)
LET negate = UNARY OP #(~) OVER Number -> Number = (operands)";
    with_program(source, scalars, nulls, |program| {
        let (types, scratch, symbols) = (program.types, program.scratch, program.symbols);
        let typed = |name| {
            let form = program
                .birth(name)
                .form()
                .expect("a callable body sits in a form");
            callable_type(form, program.activation, types, scratch).map(|callable| callable.ktype)
        };
        let mapped = |name| {
            let form = program
                .birth(name)
                .form()
                .expect("a callable body sits in a form");
            callable_type(form, program.activation, types, scratch)
                .expect("the definition elaborates")
                .quantifier_map
                .to_vec()
        };
        let registered = |name| {
            let form = program
                .birth(name)
                .form()
                .expect("a callable body sits in a form");
            callable_type(form, program.activation, types, scratch)
                .expect("the definition elaborates")
                .registered
        };
        let x = BinderSymbol::classify("x").unwrap();
        let ys = BinderSymbol::classify("ys").unwrap();
        let left = BinderSymbol::classify("left").unwrap();
        let right = BinderSymbol::classify("right").unwrap();
        let operands = BinderSymbol::classify("operands").unwrap();
        assert_eq!(
            typed("f"),
            Ok(types
                .function_type(
                    scratch,
                    &[],
                    &[(x, KType::NUMBER), (ys, types.list(KType::STR))],
                    KType::BOOL
                )
                .handle)
        );
        let shape = |elements: &[DispatchTokenElement], ret| {
            types.shape_type(scratch, &[], elements, ret).handle
        };
        let number = DispatchTokenElement::Slot(KType::NUMBER);
        // Every definition is typed by its function type, over its head's **slot names**: a call
        // through the `LET` name is by name. The registration carries the head's shape, which the
        // dispatch bucket holds.
        assert_eq!(
            typed("twice"),
            Ok(types
                .function_type(scratch, &[], &[(x, KType::NUMBER)], KType::NUMBER)
                .handle)
        );
        let elt = program.type_name("Elt");
        let quantified = types.quantified(0, KType::ANY);
        let identity = types
            .function_type(scratch, &[elt], &[(x, quantified)], quantified)
            .handle;
        assert_eq!(
            typed("id"),
            Ok(identity),
            "a quantified combined form carries its group onto the function type"
        );
        assert_eq!(
            typed("lambda_id"),
            Ok(identity),
            "the `FN FOR ALL` lambda spells the same type its combined twin does"
        );
        assert_eq!(
            mapped("lambda_id"),
            vec![(elt, Canonical::At(0))],
            "the one declared name survives canonical form at index 0, keyed by the name written"
        );
        let binary = |ret| {
            types
                .function_type(
                    scratch,
                    &[],
                    &[(left, KType::NUMBER), (right, KType::NUMBER)],
                    ret,
                )
                .handle
        };
        assert_eq!(
            typed("plus"),
            Ok(binary(KType::NUMBER)),
            "a binary operator with no result folds to its operand"
        );
        assert_eq!(typed("less"), Ok(binary(KType::BOOL)));
        assert_eq!(
            typed("negate"),
            Ok(types
                .function_type(
                    scratch,
                    &[],
                    &[(operands, types.list(KType::NUMBER))],
                    KType::NUMBER
                )
                .handle),
            "a unary operator's body takes the whole run as a list"
        );
        assert_eq!(registered("f"), None, "no bucket holds a `FN`");
        assert_eq!(registered("lambda_id"), None);
        assert_eq!(
            registered("twice"),
            Some(shape(&[keyword("TWICE", symbols), number], KType::NUMBER))
        );
        assert_eq!(
            registered("id"),
            Some(
                types
                    .shape_type(
                        scratch,
                        &[elt],
                        &[
                            keyword("ID", symbols),
                            DispatchTokenElement::Slot(quantified)
                        ],
                        quantified
                    )
                    .handle
            )
        );
        assert_eq!(
            registered("plus"),
            Some(shape(
                &[number, keyword("+", symbols), number],
                KType::NUMBER
            ))
        );
        assert_eq!(
            registered("less"),
            Some(shape(&[number, keyword("<", symbols), number], KType::BOOL))
        );
        assert_eq!(
            registered("negate"),
            Some(shape(
                &[
                    keyword("~", symbols),
                    DispatchTokenElement::Slot(types.list(KType::NUMBER))
                ],
                KType::NUMBER
            ))
        );
    });
}

/// The names `body` binds as parameters, in slot order.
fn parameters(body: &BodyShape<'_>) -> Vec<BinderSymbol> {
    (0..body.slots())
        .map(|slot| body.slot_name(Slot(slot as u32)))
        .filter(|name| body.slot(*name).map(|(_, at)| at) == Some(Position::PARAMETER))
        .collect()
}

#[test]
fn a_bare_definition_is_typed_as_its_combined_twin_is() {
    let source = "\
EXPR FOR ALL (Elt) (WHICH x :Elt ys :(LIST OF Elt)) -> Elt = (x)
LET which = FN EXPR FOR ALL (Elt) (WHICH x :Elt ys :(LIST OF Elt)) -> Elt = (x)
EXPR (TWICE x :Number) -> Number = (x)
LET twice = FN EXPR (TWICE x :Number) -> Number = (x)
OP #(*) OVER Number = (left)
LET times = OP #(*) OVER Number = (left)
UNARY OP #(~) OVER Number -> Number = (operands)
LET negate = UNARY OP #(~) OVER Number -> Number = (operands)";
    with_program(source, scalars, nulls, |program| {
        let (types, scratch) = (program.types, program.scratch);
        // A bare definition has no binder, so its body is reached by the site of its last part.
        let bare = |line: usize| {
            let body = &program.lines[line]
                .parts
                .last()
                .expect("a definition has a body")
                .value;
            program
                .activation
                .shape()
                .nested(Site::of(body))
                .expect("a bare definition's body is shaped")
        };
        let callable = |body| {
            let form = BodyShape::form(body).expect("a callable body sits in a form");
            let callable = callable_type(form, program.activation, types, scratch)
                .expect("the definition elaborates");
            (
                callable.ktype,
                callable.quantifier_map.to_vec(),
                callable.registered,
            )
        };
        for (line, name) in [(0, "which"), (2, "twice"), (4, "times"), (6, "negate")] {
            let (bare, combined) = (bare(line), program.birth(name));
            let twin = callable(combined);
            assert_eq!(callable(bare), twin, "`{name}` and its bare twin");
            assert!(twin.2.is_some(), "`{name}` registers a shape");
            assert_eq!(parameters(bare), parameters(combined), "`{name}`'s slots");
        }
        let elt = program.type_name("Elt");
        let quantified = types.quantified(0, KType::ANY);
        let [x, ys, left, right] = ["x", "ys", "left", "right"]
            .map(|name| BinderSymbol::classify(name).expect("a binder name"));
        assert_eq!(
            callable(bare(0)).0,
            types
                .function_type(
                    scratch,
                    &[elt],
                    &[(x, quantified), (ys, types.list(quantified))],
                    quantified
                )
                .handle
        );
        assert_eq!(
            callable(bare(4)).0,
            types
                .function_type(
                    scratch,
                    &[],
                    &[(left, KType::NUMBER), (right, KType::NUMBER)],
                    KType::NUMBER
                )
                .handle
        );
    });
}

/// The scalars and the value family's top, which a bound most often names.
fn with_value(
    _: &TypeRegistry<'_>,
    _: BumpAllocator<'_>,
    _: &SymbolInterner,
) -> Vec<(&'static str, KType)> {
    vec![("Value", KType::ANY_VALUE)]
}

/// Each variable's bound in `handle`'s own group, sorted, so a canonical order the test does not
/// pin reads the same either way.
fn sorted_bounds(types: &TypeRegistry<'_>, handle: KType) -> Vec<KType> {
    let mut bounds = crate::type_lattice::quantifier_bounds(types, handle).to_vec();
    bounds.sort();
    bounds
}

#[test]
fn a_bounded_quantifier_carries_its_bound() {
    let source = "\
LET Pair = :(FN FOR ALL ((Elt UNDER Value) Key) :{a :Elt b :Key c :Elt d :Key} -> Elt)
LET Shape = :(EXPR FOR ALL ((Elt UNDER Value) Key) (PAIR a :Elt b :Key c :Elt d :Key) -> Elt)
LET Lone = :(FN FOR ALL (Elt UNDER Value) :{x :Elt y :Elt} -> Elt)
LET Doubled = :(FN FOR ALL ((Elt UNDER Value)) :{x :Elt y :Elt} -> Elt)
LET Free = :(FN FOR ALL (Elt) :{x :Elt y :Elt} -> Elt)
LET Spanning = :(FN FOR ALL (Elt UNDER :(Number | Str | Bool)) :{x :Elt y :Elt} -> Elt)";
    with_program(source, with_value, nulls, |program| {
        let (types, scratch) = (program.types, program.scratch);
        let mut expected = vec![KType::ANY_VALUE, KType::ANY];
        expected.sort();
        for line in [0, 1] {
            let handle = elaborated(&program, line).expect("the type elaborates");
            assert_eq!(sorted_bounds(types, handle), expected, "line {line}");
        }
        let lone = elaborated(&program, 2).expect("the type elaborates");
        assert_eq!(sorted_bounds(types, lone), vec![KType::ANY_VALUE]);
        assert_eq!(
            elaborated(&program, 3),
            Ok(lone),
            "one bounded name, however parenthesized"
        );
        assert_ne!(elaborated(&program, 4), Ok(lone), "a bound is identity");
        let spanning = elaborated(&program, 5).expect("the type elaborates");
        assert_eq!(
            sorted_bounds(types, spanning),
            vec![types.union_of(scratch, &[KType::NUMBER, KType::STR, KType::BOOL])]
        );
    });
}

#[test]
fn a_meet_is_the_greatest_lower_bound_of_its_operands() {
    let source = "\
LET Met = :((Number | Str) & (Str | Bool))
LET Chained = :((Number | Str | Bool) & (Str | Bool | Null) & (Bool | Number))
LET Fields = :(:{x :Number} & :{y :Str})
LET Disjoint = :(Number & Str)";
    with_program(source, scalars, nulls, |program| {
        let (types, scratch) = (program.types, program.scratch);
        let x = BinderSymbol::classify("x").unwrap();
        let y = BinderSymbol::classify("y").unwrap();
        assert_eq!(elaborated(&program, 0), Ok(KType::STR));
        assert_eq!(elaborated(&program, 1), Ok(KType::BOOL));
        assert_eq!(
            elaborated(&program, 2),
            Ok(types.record(scratch, &[(x, KType::NUMBER), (y, KType::STR)]))
        );
        assert_eq!(elaborated(&program, 3), Ok(KType::NEVER));
    });
}

#[test]
fn a_bound_naming_a_variable_or_never_is_refused() {
    let source = "\
LET Own = :(FN FOR ALL ((Elt UNDER Key) Key) :{x :Elt y :Key} -> Elt)
LET Empty = :(FN FOR ALL (Elt UNDER :(Number & Str)) :{x :Elt y :Elt} -> Elt)
LET Over = :(FN FOR ALL ((Elt OVER Value)) :{x :Number} -> Number)
LET Nested = :(FN FOR ALL (Outer) :{f :(FN FOR ALL (Elt UNDER Outer) :{x :Elt y :Elt} -> Elt) g :Outer} -> Outer)";
    with_program(source, with_value, nulls, |program| {
        assert!(matches!(
            elaborated(&program, 0),
            Err(Elaboration::Bound { .. })
        ));
        assert!(matches!(
            elaborated(&program, 1),
            Err(Elaboration::Bound { .. })
        ));
        assert!(matches!(
            elaborated(&program, 2),
            Err(Elaboration::Unsupported { .. })
        ));
        assert!(matches!(
            elaborated(&program, 3),
            Err(Elaboration::Unsupported { .. })
        ));
    });
}

#[test]
fn a_callable_carries_its_bounds_and_maps_a_dropped_name_to_its_bound() {
    let source = "\
LET lambda = (FN FOR ALL (Elt UNDER Number) :{x :Elt y :Elt} -> Elt = (x))
LET id = FN EXPR FOR ALL (Elt UNDER Number) (ID x :Elt) -> Elt = (x)
EXPR FOR ALL (Elt UNDER Number) (TWICE x :Elt y :Elt) -> Elt = (x)
LET which = (FN FOR ALL ((Unused UNDER Value) Held) :{x :(LIST OF Held)} -> Held = (Unused))";
    with_program(source, with_value, nulls, |program| {
        let (types, scratch) = (program.types, program.scratch);
        let twice = program
            .activation
            .shape()
            .nested(Site::of(&program.lines[2].parts[8].value))
            .expect("the definition births a body");
        for body in [program.birth("lambda"), program.birth("id"), twice] {
            let form = body.form().expect("a callable body sits in a form");
            let ktype = callable_type(form, program.activation, types, scratch)
                .expect("it elaborates")
                .ktype;
            assert_eq!(sorted_bounds(types, ktype), vec![KType::NUMBER]);
        }
        let form = program.birth("which").form().expect("a form");
        let which = callable_type(form, program.activation, types, scratch).expect("it elaborates");
        assert_eq!(
            which.quantifier_map.to_vec(),
            vec![
                (
                    program.type_name("Unused"),
                    Canonical::Dropped {
                        bound: KType::ANY_VALUE
                    }
                ),
                (program.type_name("Held"), Canonical::At(0)),
            ]
        );
    });
}
