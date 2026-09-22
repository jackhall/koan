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
            callable_type(form, program.activation, types, scratch)
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
        assert_eq!(
            typed("twice"),
            Ok(shape(&[keyword("TWICE", symbols), number], KType::NUMBER))
        );
        let elt = program.type_name("Elt");
        let quantified = types.quantified(0, KType::ANY);
        assert_eq!(
            typed("id"),
            Ok(types
                .shape_type(
                    scratch,
                    &[elt],
                    &[
                        keyword("ID", symbols),
                        DispatchTokenElement::Slot(quantified)
                    ],
                    quantified
                )
                .handle)
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
