//! Each production a type expression elaborates, each refusal, and a callable's type read off each
//! form that births one.

use crate::memory::BumpAllocator;
use crate::parse::{ExpressionPart, KExpression};
use crate::scope::{BodyShape, Position, Site, Slot, Which};
use crate::symbols::{BinderSymbol, KeywordSymbol, SymbolInterner};
use crate::type_lattice::{
    DispatchTokenElement, KType, RecursiveGroupWindow, RelativeSchema, TypeRegistry, display_name,
};
use crate::values::Value;

use super::super::{callable_type, type_expression};
use super::{Held, Program, nulls, scalars, with_program};
use crate::scope::{Elaboration, ParameterBinding};

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
LET Headed = :(EXPR #(TWICE x :Number) -> Number)
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
                    .function_type(scratch, &[], &[], &[(x, KType::NUMBER)], KType::BOOL)
                    .handle)
            );
            let twice = [
                keyword("TWICE", program.symbols),
                DispatchTokenElement::Slot(KType::NUMBER),
            ];
            assert_eq!(
                elaborated(&program, 6),
                Ok(types
                    .shape_type(scratch, &[], &[], &twice, &[], KType::NUMBER)
                    .handle)
            );
            assert_eq!(elaborated(&program, 7), Ok(KType::STR));
        },
    );
}

#[test]
fn a_ranked_head_interns_by_its_dense_ranking() {
    // A signature member ranks a head by the integers in its slots' places: `2 … 1` and `20 … 10`
    // are one ranking, written order another, and a named slot sits in the unnumbered class.
    let source = "\
LET Ranked = :(EXPR #(MOVE 2 :Number TO 1 :Str) -> Number)
LET Scaled = :(EXPR #(MOVE 20 :Number TO 10 :Str) -> Number)
LET Written = :(EXPR #(MOVE _ :Number TO _ :Str) -> Number)
LET Named = :(EXPR #(MOVE piece :Number TO 1 :Str) -> Number)";
    with_program(source, scalars, nulls, |program| {
        let (types, scratch) = (program.types, program.scratch);
        let elements = [
            keyword("MOVE", program.symbols),
            DispatchTokenElement::Slot(KType::NUMBER),
            keyword("TO", program.symbols),
            DispatchTokenElement::Slot(KType::STR),
        ];
        let ranked = types
            .shape_type(scratch, &[], &[], &elements, &[1, 0], KType::NUMBER)
            .handle;
        assert_eq!(elaborated(&program, 0), Ok(ranked));
        assert_eq!(elaborated(&program, 1), Ok(ranked));
        assert_ne!(elaborated(&program, 2), Ok(ranked));
        assert_eq!(elaborated(&program, 3), Ok(ranked));
        assert_eq!(
            display_name(ranked, types, program.symbols).to_string(),
            ":(EXPR #(MOVE 2 :Number TO 1 :Str) -> Number)"
        );
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
LET f = (FN :{x :Number, ys :(LIST OF Str)} -> Bool = #(x))
LET twice = FN EXPR #(TWICE x :Number) -> Number = #(x)
LET id = FN EXPR FOR ALL #[Elt] #(ID x :Elt) -> Elt = #(x)
LET lambda_id = (FN FOR ALL #[Elt] :{x :Elt} -> Elt = #(x))
LET plus = OP #(+) OVER Number = #(left)
LET less = OP #(<) OVER Number -> Bool = #(left)
LET negate = UNARY OP #(~) OVER Number -> Number = #(operands)";
    with_program(source, scalars, nulls, |program| {
        let (types, scratch, symbols) = (program.types, program.scratch, program.symbols);
        let typed = |name| program.callable(name, false).map(|callable| callable.ktype);
        let mapped = |name| {
            program
                .callable(name, false)
                .expect("the definition elaborates")
                .quantifier_map
                .to_vec()
        };
        let registered = |name| {
            program
                .callable(name, true)
                .expect("the definition elaborates")
                .registered
                .expect("born for its registration")
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
                    &[],
                    &[(x, KType::NUMBER), (ys, types.list(KType::STR))],
                    KType::BOOL
                )
                .handle)
        );
        let shape = |elements: &[DispatchTokenElement], ret| {
            types
                .shape_type(scratch, &[], &[], elements, &[], ret)
                .handle
        };
        let number = DispatchTokenElement::Slot(KType::NUMBER);
        // Every definition is typed by its function type, over its head's **slot names**: a call
        // through the `LET` name is by name. The registration carries the head's shape, which the
        // dispatch bucket holds.
        assert_eq!(
            typed("twice"),
            Ok(types
                .function_type(scratch, &[], &[], &[(x, KType::NUMBER)], KType::NUMBER)
                .handle)
        );
        let elt = program.type_name("Elt");
        let quantified = types.quantified(0, KType::ANY);
        let identity = types
            .function_type(
                scratch,
                &[elt],
                &[KType::ANY],
                &[(x, quantified)],
                quantified,
            )
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
            vec![(elt, 0)],
            "the one declared name is at index 0, keyed by the name written"
        );
        let binary = |ret| {
            types
                .function_type(
                    scratch,
                    &[],
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
                    &[],
                    &[(operands, types.list(KType::NUMBER))],
                    KType::NUMBER
                )
                .handle),
            "a unary operator's body takes the whole run as a list"
        );
        assert_eq!(
            program
                .callable("twice", false)
                .map(|callable| callable.registered),
            Ok(None),
            "a callable born for its name carries no bucket's shape"
        );
        assert_eq!(
            registered("twice").shape,
            shape(&[keyword("TWICE", symbols), number], KType::NUMBER)
        );
        assert_eq!(
            registered("twice").parameters,
            ParameterBinding::Named(&[x])
        );
        assert_eq!(registered("id").quantifier_map, &[(elt, 0)][..]);
        assert_eq!(
            registered("id").shape,
            types
                .shape_type(
                    scratch,
                    &[elt],
                    &[KType::ANY],
                    &[
                        keyword("ID", symbols),
                        DispatchTokenElement::Slot(quantified)
                    ],
                    &[],
                    quantified
                )
                .handle
        );
        assert_eq!(
            registered("plus").shape,
            shape(&[number, keyword("+", symbols), number], KType::NUMBER)
        );
        assert_eq!(
            registered("plus").parameters,
            ParameterBinding::Named(&[left, right])
        );
        assert_eq!(
            registered("less").shape,
            shape(&[number, keyword("<", symbols), number], KType::BOOL)
        );
        assert_eq!(
            registered("negate").shape,
            shape(
                &[
                    keyword("~", symbols),
                    DispatchTokenElement::Slot(types.list(KType::NUMBER))
                ],
                KType::NUMBER
            )
        );
        assert_eq!(
            registered("negate").parameters,
            ParameterBinding::Named(&[operands])
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
EXPR FOR ALL #[Elt] #(WHICH x :Elt ys :(LIST OF Elt)) -> Elt = #(x)
LET which = FN EXPR FOR ALL #[Elt] #(WHICH x :Elt ys :(LIST OF Elt)) -> Elt = #(x)
EXPR #(TWICE x :Number) -> Number = #(x)
LET twice = FN EXPR #(TWICE x :Number) -> Number = #(x)
OP #(*) OVER Number = #(left)
LET times = OP #(*) OVER Number = #(left)
UNARY OP #(~) OVER Number -> Number = #(operands)
LET negate = UNARY OP #(~) OVER Number -> Number = #(operands)";
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
            let registration = Some(program.registration(body));
            let callable = callable_type(form, program.activation, types, scratch, registration)
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
                    &[KType::ANY],
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
fn a_callable_carries_its_bounds_and_maps_each_name_to_its_index() {
    let source = "\
LET lambda = (FN FOR ALL #{Elt: Number} :{x :Elt y :Elt} -> Elt = #(x))
LET id = FN EXPR FOR ALL #{Elt: Number} #(ID x :Elt) -> Elt = #(x)
EXPR FOR ALL #{Elt: Number} #(TWICE x :Elt y :Elt) -> Elt = #(x)
LET which = (FN FOR ALL #{Unused: Value, Held: Any} :{x :(LIST OF Held)} -> Held = #(Unused))";
    with_program(source, with_value, nulls, |program| {
        let (types, scratch) = (program.types, program.scratch);
        let twice = program
            .activation
            .shape()
            .nested(Site::of(&program.lines[2].parts[8].value))
            .expect("the definition births a body");
        for body in [program.birth("lambda"), program.birth("id"), twice] {
            let form = body.form().expect("a callable body sits in a form");
            let ktype = callable_type(form, program.activation, types, scratch, None)
                .expect("it elaborates")
                .ktype;
            assert_eq!(sorted_bounds(types, ktype), vec![KType::NUMBER]);
        }
        let form = program.birth("which").form().expect("a form");
        let which =
            callable_type(form, program.activation, types, scratch, None).expect("it elaborates");
        assert_eq!(
            which.quantifier_map.to_vec(),
            vec![
                (program.type_name("Unused"), 1),
                (program.type_name("Held"), 0),
            ],
            "`Held` is named first; `Unused`, named by no position, comes after"
        );
        assert_eq!(
            crate::type_lattice::quantifier_bounds(types, which.ktype),
            &[KType::ANY, KType::ANY_VALUE]
        );
    });
}

#[test]
fn a_registered_shape_takes_the_ranking_its_declaration_gives_and_the_type_spells_it() {
    let source = "\
EXPR #(MOVE 2 TO 1)
LET move = FN EXPR #(MOVE x :Number TO y :Str) -> Number = #(x)
LET ranked = :(EXPR #(MOVE 2 :Number TO 1 :Str) -> Number)
LET scaled = :(EXPR #(MOVE 20 :Number TO 10 :Str) -> Number)
LET written = :(EXPR #(MOVE _ :Number TO _ :Str) -> Number)";
    with_program(source, scalars, nulls, |program| {
        let ranked = elaborated(&program, 2).expect("a ranked head elaborates");
        assert_eq!(elaborated(&program, 3), Ok(ranked), "`20 10` is `2 1`");
        assert_ne!(
            elaborated(&program, 4),
            Ok(ranked),
            "written order is another ranking"
        );
        let registered = program
            .callable("move", true)
            .expect("the definition elaborates")
            .registered
            .expect("born for its registration");
        assert_eq!(
            registered.shape, ranked,
            "the definition ranks as the declaration it sees"
        );
        assert_eq!(
            registered.parameters,
            ParameterBinding::Named(&[
                BinderSymbol::classify("x").unwrap(),
                BinderSymbol::classify("y").unwrap()
            ])
        );
    });
}

#[test]
fn a_unary_operator_at_its_binary_key_packs_its_slots_into_operands() {
    let source = "UNARY OP #(~) OVER Number -> Number = #(operands)";
    with_program(source, scalars, nulls, |program| {
        let (types, scratch, symbols) = (program.types, program.scratch, program.symbols);
        let shape = program.activation.shape();
        let body = &program.lines[0]
            .parts
            .last()
            .expect("a definition has a body")
            .value;
        let body = shape.nested(Site::of(body)).expect("the body is shaped");
        let bridge = shape
            .registrations()
            .iter()
            .find(|registration| registration.which == Which::Binary)
            .expect("a unary operator registers under a binary key too");
        let form = body.form().expect("a callable body sits in a form");
        let registered = callable_type(form, program.activation, types, scratch, Some(bridge))
            .expect("the operator elaborates")
            .registered
            .expect("born for its registration");
        let number = DispatchTokenElement::Slot(KType::NUMBER);
        assert_eq!(
            registered.shape,
            types
                .shape_type(
                    scratch,
                    &[],
                    &[],
                    &[number, keyword("~", symbols), number],
                    &[],
                    KType::NUMBER
                )
                .handle
        );
        assert_eq!(registered.parameters, ParameterBinding::Operands);
    });
}
