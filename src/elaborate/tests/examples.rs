//! Each production a type expression elaborates, each refusal, and a callable's type read off each
//! form that births one.

use crate::memory::BumpAllocator;
use crate::parse::{ExpressionPart, KExpression};
use crate::scope::Site;
use crate::symbols::{BinderSymbol, KeywordSymbol, SymbolInterner};
use crate::type_lattice::{
    DispatchTokenElement, KType, RecursiveGroupWindow, RelativeSchema, TypeRegistry,
};
use crate::values::Value;

use super::super::{Elaboration, callable_type, type_expression};
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
        &[],
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
            Err(Elaboration::NoSuchMember { tag, .. }) if tag == program.type_name("Triangle").symbol()
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
        let x = BinderSymbol::classify("x").unwrap();
        let ys = BinderSymbol::classify("ys").unwrap();
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
        // A combined form's type is the function over its head's **slot names**, not the head's
        // shape: a call through the `LET` name is by name. Only the dispatch bucket carries the
        // shape.
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
            vec![Some(0)],
            "the one declared name survives canonical form at index 0"
        );
        assert_eq!(
            typed("plus"),
            Ok(shape(
                &[number, keyword("+", symbols), number],
                KType::NUMBER
            )),
            "a binary operator with no result folds to its operand"
        );
        assert_eq!(
            typed("less"),
            Ok(shape(&[number, keyword("<", symbols), number], KType::BOOL))
        );
        assert_eq!(
            typed("negate"),
            Ok(shape(
                &[
                    keyword("~", symbols),
                    DispatchTokenElement::Slot(types.list(KType::NUMBER))
                ],
                KType::NUMBER
            )),
            "a unary operator's body takes the whole run as a list"
        );
    });
}
