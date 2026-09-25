//! Parameterized unions and the families they declare: each variant a family over the union's
//! parameters, the builtin `Result` one of them, a union head applied per variant, a construction
//! through a variant, a recursive family, and the declarations the door refuses. The harness has no
//! `Never` builtin, so expected applications are built here.

use crate::symbols::{BinderSymbol, TypeSymbol};
use crate::type_lattice::{KKind, KType, NodeSchema, TypeNode, is_subtype_of};
use crate::values::construction;

use super::super::{Elaboration, builtin_result};
use super::{Program, brought, declared};

const RESULT: &str = "UNION (Ok Error AS Result) = (Ok :Ok Error :Error)";
const OPTION: &str = "UNION (Elem AS Option) = (Some :Elem None :Null)";
const TREE: &str = "UNION (Elem AS Tree) = (Leaf :Null Node :{value :Elem, left :(Elem AS Tree), right :(Elem AS Tree)})";

/// The variant `tag` of the union `binder` is bound to.
fn variant(program: &Program<'_, '_, '_>, binder: &str, tag: &str) -> KType {
    program
        .types
        .union_member_named(program.bound(binder), program.type_name(tag).symbol())
        .unwrap_or_else(|| panic!("`{binder}` declares `{tag}`"))
}

/// A family member's representation and parameter names.
fn family<'p>(program: &Program<'p, '_, '_>, handle: KType) -> (Option<KType>, &'p [TypeSymbol]) {
    match program.types.node(handle) {
        TypeNode::SetMember {
            kind: KKind::TypeConstructor,
            schema:
                NodeSchema::TypeConstructor {
                    representation,
                    param_names,
                },
            ..
        } => (representation, param_names),
        _ => panic!("the handle is a family member"),
    }
}

/// `constructor` applied to the named arguments.
fn applied(
    program: &Program<'_, '_, '_>,
    constructor: KType,
    arguments: &[(&str, KType)],
) -> KType {
    let arguments: Vec<_> = arguments
        .iter()
        .map(|(name, ktype)| (BinderSymbol::Type(program.type_name(name)), *ktype))
        .collect();
    program
        .types
        .constructor_apply(program.scratch, constructor, &arguments)
}

/// The quantifier a family's representation reads for its parameter `name`.
fn parameter(program: &Program<'_, '_, '_>, params: &[TypeSymbol], name: &str) -> KType {
    let index = params
        .iter()
        .position(|param| *param == program.type_name(name))
        .expect("a declared parameter");
    program.types.quantified(index, KType::ANY)
}

#[test]
fn a_parameterized_union_declares_a_family_per_variant() {
    brought(
        &format!("{RESULT}\nUNION Shape = (Circle :Number)"),
        |program| {
            let TypeNode::Union { members } = program.types.node(program.bound("Result")) else {
                panic!("a UNION binds a union");
            };
            assert_eq!(members.len(), 2);
            let mut sorted = [program.type_name("Ok"), program.type_name("Error")];
            sorted.sort_unstable();
            for tag in ["Ok", "Error"] {
                let (representation, params) = family(&program, variant(&program, "Result", tag));
                assert_eq!(
                    params, sorted,
                    "each variant is a family over both parameters"
                );
                assert_eq!(representation, Some(parameter(&program, params, tag)));
            }
            assert!(
                matches!(
                    program.types.node(program.bound("Shape")),
                    TypeNode::SetMember {
                        kind: KKind::NewType,
                        ..
                    }
                ),
                "a union with no parameters declares newtype variants"
            );
        },
    );
}

#[test]
fn the_builtin_result_is_the_declared_one() {
    brought(RESULT, |program| {
        assert_eq!(
            builtin_result(program.types, program.symbols, program.scratch),
            program.bound("Result")
        );
    });
}

#[test]
fn a_union_head_applies_per_variant() {
    brought(
        &format!(
            "{RESULT}\n{OPTION}\nLET Both = :(Result {{Ok = Number, Error = Str}})\n\
             LET NumberOption = :(Number AS Option)\n\
             LET Just = :(Result.Ok {{Ok = Number, Error = Str}})"
        ),
        |program| {
            let arguments = [("Ok", KType::NUMBER), ("Error", KType::STR)];
            let ok = applied(&program, variant(&program, "Result", "Ok"), &arguments);
            let error = applied(&program, variant(&program, "Result", "Error"), &arguments);
            assert_eq!(
                program.bound("Both"),
                program.types.union_of(program.scratch, &[ok, error])
            );
            assert_eq!(program.bound("Just"), ok, "a variant head applies alone");
            let some = applied(
                &program,
                variant(&program, "Option", "Some"),
                &[("Elem", KType::NUMBER)],
            );
            let none = applied(
                &program,
                variant(&program, "Option", "None"),
                &[("Elem", KType::NUMBER)],
            );
            assert_eq!(
                program.bound("NumberOption"),
                program.types.union_of(program.scratch, &[some, none])
            );
        },
    );
}

#[test]
fn constructing_through_a_family_solves_its_parameters() {
    brought(
        &format!(
            "{RESULT}\n{OPTION}\nNEWTYPE (Type AS Boxed)\n\
             LET Both = :(Result {{Ok = Number, Error = Str}})\n\
             LET Strings = :(Result {{Ok = Str, Error = Str}})\n\
             LET NumberOption = :(Number AS Option)\nLET BoxedNumber = :(Number AS Boxed)"
        ),
        |program| {
            let (types, scratch) = (program.types, program.scratch);
            let below = |a, b| is_subtype_of(types, scratch, a, b);

            let ok = variant(&program, "Result", "Ok");
            let built = construction(types, scratch, ok, KType::NUMBER).expect("`Ok` constructs");
            assert_eq!(
                built,
                applied(
                    &program,
                    ok,
                    &[("Ok", KType::NUMBER), ("Error", KType::NEVER)]
                )
            );
            assert!(below(built, program.bound("Both")));
            assert!(!below(built, program.bound("Strings")));
            assert!(below(built, program.bound("Result")));

            let boxed = program.bound("Boxed");
            let built = construction(types, scratch, boxed, KType::NUMBER).expect("`Boxed` wraps");
            assert_eq!(built, program.bound("BoxedNumber"));
            assert!(below(built, boxed));

            let none = variant(&program, "Option", "None");
            let built = construction(types, scratch, none, KType::NULL).expect("`None` constructs");
            assert_eq!(built, applied(&program, none, &[("Elem", KType::NEVER)]));
            assert!(below(built, program.bound("NumberOption")));
        },
    );
}

#[test]
fn a_tree_is_a_recursive_family() {
    brought(TREE, |program| {
        let (types, scratch) = (program.types, program.scratch);
        let (leaf, node) = (
            variant(&program, "Tree", "Leaf"),
            variant(&program, "Tree", "Node"),
        );
        let (Some(representation), params) = family(&program, node) else {
            panic!("`Node` constructs");
        };
        let TypeNode::Record { fields } = types.node(representation) else {
            panic!("`Node` wraps a record");
        };
        let elem = parameter(&program, params, "Elem");
        let subtree = types.union_of(
            scratch,
            &[
                applied(&program, leaf, &[("Elem", elem)]),
                applied(&program, node, &[("Elem", elem)]),
            ],
        );
        let left = BinderSymbol::classify("left").unwrap();
        assert_eq!(fields.get(left.symbol()), Some(subtree));

        let empty = construction(types, scratch, leaf, KType::NULL).expect("`Leaf` constructs");
        let field = |name| BinderSymbol::classify(name).unwrap();
        let payload = types.record(
            scratch,
            &[
                (field("value"), KType::NUMBER),
                (field("left"), empty),
                (field("right"), empty),
            ],
        );
        assert_eq!(
            construction(types, scratch, node, payload),
            Ok(applied(&program, node, &[("Elem", KType::NUMBER)]))
        );
    });
    brought(
        "UNION (Elem AS Tree) = (Leaf :Null Branch :{value :Elem, kids :(Elem AS Forest)})\n\
         UNION (Elem AS Forest) = (Nil :Null Cons :{head :(Elem AS Tree), tail :(Elem AS Forest)})",
        |program| {
            let (_, params) = family(&program, variant(&program, "Forest", "Cons"));
            assert_eq!(params, [program.type_name("Elem")]);
        },
    );
}

#[test]
fn a_family_declaration_is_refused() {
    // Each case names the binder whose slot the refusal must leave empty.
    for (source, left) in [
        // An application is covariant in its arguments.
        (
            "UNION (Elem AS Sink) = (Take :(FN :{x :Elem} -> Null))",
            "Sink",
        ),
        ("UNION (Elem Elem AS Twice) = (One :Elem)", "Twice"),
        // A family's parameters take no bound.
        ("UNION ((Elem AS Opt) UNDER Any) = (None :Null)", "Opt"),
        (
            &format!("{RESULT}\nLET Bad = :(Result {{Ok = Number}})"),
            "Bad",
        ),
        (
            &format!("{RESULT}\nLET Bad = :(Result {{Ok = Number, Error = Str, Other = Null}})"),
            "Bad",
        ),
        // A nested group shadows the union's parameters.
        (
            "UNION (Elem AS Poly) = (Map :(FN FOR ALL (Held) :{x :Held} -> Elem))",
            "Poly",
        ),
    ] {
        declared(source, |program, brought| {
            assert!(
                matches!(brought, Err(Elaboration::Unsupported { .. })),
                "`{source}` refuses: {brought:?}"
            );
            assert!(program.unbound(left), "`{source}` leaves `{left}` empty");
        });
    }
    // A parameter at a covariant position declares, however many flips put it there.
    for source in [
        "UNION (Elem AS Source) = (Give :(FN :{} -> Elem))",
        "UNION (Elem AS Wrapper) = (Nest :(FN :{f :(FN :{x :Elem} -> Null)} -> Null))",
    ] {
        brought(source, |_| ());
    }
}
