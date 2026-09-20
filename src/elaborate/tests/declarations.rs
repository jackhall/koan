//! The door a component of type binders comes into being through: each declaration it elaborates,
//! each group it seals, and each refusal.

use crate::parse::{BinderSymbol, ValueSymbol};
use crate::type_lattice::{KKind, KType, NodeSchema, TypeNode, member};

use super::super::{Elaboration, callable_type};
use super::{Held, Program, scalars, with_program};

/// Shape and activate `source` with every slot claimed, run the door over every component of type
/// binders, and hand the result to `check`.
fn declared<R>(
    source: &str,
    check: impl for<'p, 'graph, 'cell> FnOnce(Program<'p, 'graph, 'cell>, Result<(), Elaboration>) -> R,
) -> R {
    with_program(
        source,
        scalars,
        |_, _, _| Held::Pending,
        |program| {
            let brought = program.declare();
            check(program, brought)
        },
    )
}

/// `source` declares its types, or the test fails with the refusal.
fn brought<R>(source: &str, check: impl for<'p, 'graph, 'cell> FnOnce(Program<'p, 'graph, 'cell>) -> R) -> R {
    declared(source, |program, brought| {
        brought.unwrap_or_else(|refusal| panic!("`{source}` declares: {refusal:?}"));
        check(program)
    })
}

/// The representation a newtype member wraps.
fn representation(program: &Program<'_, '_, '_>, handle: KType) -> KType {
    match program.types.node(handle) {
        TypeNode::SetMember {
            kind: KKind::NewType,
            schema: NodeSchema::NewType(repr),
            ..
        } => repr,
        _ => panic!("the handle is a newtype member"),
    }
}

#[test]
fn a_lone_newtype_mints_a_fresh_nominal_over_its_representation() {
    brought(
        "NEWTYPE Distance = Number\nNEWTYPE Point = :{x :Number, y :Number}",
        |program| {
            let distance = program.bound("Distance");
            assert_ne!(distance, KType::NUMBER, "a newtype is not its own repr");
            assert_eq!(representation(&program, distance), KType::NUMBER);
            let x = BinderSymbol::classify("x").unwrap();
            let y = BinderSymbol::classify("y").unwrap();
            let fields = program
                .types
                .record(program.scratch, &[(x, KType::NUMBER), (y, KType::NUMBER)]);
            assert_eq!(representation(&program, program.bound("Point")), fields);
        },
    );
}

#[test]
fn a_union_binds_the_canonical_union_of_its_variants() {
    brought("UNION Maybe = (Some :Number None :Null)", |program| {
        let maybe = program.bound("Maybe");
        let TypeNode::Union { members } = program.types.node(maybe) else {
            panic!("a UNION binds a union");
        };
        assert_eq!(members.len(), 2);
        let some = program
            .types
            .union_member_named(maybe, program.type_name("Some").symbol())
            .expect("the union declares `Some`");
        assert_eq!(representation(&program, some), KType::NUMBER);
    });
}

#[test]
fn an_alias_and_a_signature_are_each_their_own_member() {
    brought("LET Alias = Number\nSIG HasLabel = (VAL label :Str)", |program| {
        assert_eq!(program.bound("Alias"), KType::NUMBER);
        let TypeNode::Signature { schema, .. } = program.types.node(program.bound("HasLabel"))
        else {
            panic!("a SIG binds a signature");
        };
        let label = ValueSymbol::declared("label", program.labels).unwrap();
        assert_eq!(member(schema.value_slots, label), Some(KType::STR));
    });
}

#[test]
fn two_declarations_of_the_same_shape_intern_equal() {
    // Identity is content, so the same source in two programs — and a ring written in either
    // order — reaches one handle.
    let sig = |source| brought(source, |program| program.bound("HasLabel"));
    assert_eq!(
        sig("SIG HasLabel = (VAL label :Str)"),
        sig("SIG HasLabel = (VAL label :Str)")
    );
    let ring = |source| brought(source, |program| program.bound("Aa"));
    assert_eq!(
        ring("NEWTYPE Aa = :{b :Bb}\nNEWTYPE Bb = :{a :Aa}"),
        ring("NEWTYPE Bb = :{a :Aa}\nNEWTYPE Aa = :{b :Bb}"),
        "one SCC digest, whichever order the ring is written in"
    );
}

#[test]
fn a_member_of_a_sealed_group_reads_its_fellows() {
    brought("NEWTYPE Ring = :{next :Ring}", |program| {
        let ring = program.bound("Ring");
        let TypeNode::Record { fields } = program.types.node(representation(&program, ring)) else {
            panic!("the representation is a record");
        };
        let next = BinderSymbol::classify("next").unwrap();
        assert_eq!(fields.get(next.symbol()), Some(ring), "the field names the member");
    });
    brought("UNION Tree = (Node :Tree Leaf :Null)", |program| {
        let tree = program.bound("Tree");
        let node = program
            .types
            .union_member_named(tree, program.type_name("Node").symbol())
            .expect("the union declares `Node`");
        assert_eq!(
            representation(&program, node),
            tree,
            "a variant payload naming its own binder reads the union"
        );
    });
}

#[test]
fn a_union_and_a_newtype_seal_in_one_group() {
    brought(
        "UNION Nat = (Zero :Null Succ :Nat)\nNEWTYPE Wrap = :{n :Nat}",
        |program| {
            let nat = program.bound("Nat");
            let wrap = program.bound("Wrap");
            let TypeNode::Record { fields } = program.types.node(representation(&program, wrap))
            else {
                panic!("the representation is a record");
            };
            let n = BinderSymbol::classify("n").unwrap();
            assert_eq!(fields.get(n.symbol()), Some(nat));
        },
    );
}

#[test]
fn a_signature_declares_its_abstract_and_manifest_members() {
    brought(
        "SIG Fixed = ((TYPE Carrier) (LET Elem = Number) (VAL x :Elem) (VAL c :Carrier))",
        |program| {
            let TypeNode::Signature { schema, .. } = program.types.node(program.bound("Fixed"))
            else {
                panic!("a SIG binds a signature");
            };
            let carrier = program.type_name("Carrier");
            let rigid = member(schema.abstract_members, carrier)
                .expect("the signature declares `Carrier`");
            assert!(matches!(
                program.types.node(rigid),
                TypeNode::AbstractType { .. }
            ));
            assert_eq!(
                member(schema.manifest_members, program.type_name("Elem")),
                Some(KType::NUMBER),
                "a manifest member fixes its type"
            );
            let slot = |name| {
                member(
                    schema.value_slots,
                    ValueSymbol::declared(name, program.labels).unwrap(),
                )
            };
            assert_eq!(slot("x"), Some(KType::NUMBER), "read through the locals");
            assert_eq!(slot("c"), Some(rigid));
        },
    );
}

#[test]
fn a_signatures_bodyless_heads_are_keyworded_members() {
    brought(
        "SIG Ring = ((TYPE Carrier) (OP #(+) OVER Carrier) (UNARY OP #(-) OVER Carrier -> Carrier))",
        |program| {
            let TypeNode::Signature { schema, .. } = program.types.node(program.bound("Ring"))
            else {
                panic!("a SIG binds a signature");
            };
            assert_eq!(schema.keyworded.len(), 2);
            assert!(
                schema.operators.is_empty(),
                "the operator channel is operator-groups', not this door's"
            );
        },
    );
}

#[test]
fn a_bodyless_head_spells_the_shape_its_definition_spells() {
    // One builder reads both, so a head and the definition satisfying it can never disagree.
    brought(
        "SIG Arith = ((EXPR (TWICE x :Number) -> Number) (OP #(+) OVER Number) \
                      (UNARY OP #(-) OVER Number -> Number))\n\
         LET twice = FN EXPR (TWICE x :Number) -> Number = (x)\n\
         LET plus = OP #(+) OVER Number = (left)\n\
         LET negate = UNARY OP #(-) OVER Number -> Number = (operands)",
        |program| {
            let TypeNode::Signature { schema, .. } = program.types.node(program.bound("Arith"))
            else {
                panic!("a SIG binds a signature");
            };
            let defined = |name| {
                let form = program
                    .birth(name)
                    .form()
                    .expect("a callable body sits in a form");
                callable_type(form, program.activation, program.types, program.scratch)
                    .expect("the definition elaborates")
            };
            let mut declared: Vec<KType> = schema.keyworded.to_vec();
            let mut satisfiers = vec![defined("twice"), defined("plus"), defined("negate")];
            declared.sort_unstable();
            satisfiers.sort_unstable();
            assert_eq!(declared, satisfiers);
        },
    );
}

#[test]
fn a_declared_family_is_applied_by_member_name() {
    brought(
        "NEWTYPE (Key Val AS Pair)\nNEWTYPE (Held AS Boxed)\n\
         LET NumToStr = :(Pair {Key = Number, Val = Str})\nLET BoxedNum = :(Number AS Boxed)",
        |program| {
            let pair = program.bound("Pair");
            assert!(matches!(
                program.types.node(pair),
                TypeNode::SetMember {
                    kind: KKind::TypeConstructor,
                    ..
                }
            ));
            let applied = program.bound("NumToStr");
            let TypeNode::ConstructorApply {
                constructor,
                arguments,
            } = program.types.node(applied)
            else {
                panic!("an application interns a ConstructorApply");
            };
            assert_eq!(constructor, pair);
            assert_eq!(arguments.len(), 2);
            let TypeNode::ConstructorApply { constructor, .. } =
                program.types.node(program.bound("BoxedNum"))
            else {
                panic!("the arity-one sugar applies too");
            };
            assert_eq!(constructor, program.bound("Boxed"));
        },
    );
}

#[test]
fn a_signatures_higher_kinded_member_has_a_use_site() {
    brought(
        "SIG Boxy = ((TYPE (Held AS Boxed)) (VAL unbox :(FN :{x :(Number AS Boxed)} -> Number)))",
        |program| {
            let TypeNode::Signature { schema, .. } = program.types.node(program.bound("Boxy"))
            else {
                panic!("a SIG binds a signature");
            };
            let boxed = member(schema.abstract_members, program.type_name("Boxed"))
                .expect("the signature declares `Boxed`");
            let TypeNode::AbstractType { param_names, .. } = program.types.node(boxed) else {
                panic!("a higher-kinded member is an abstract type");
            };
            assert_eq!(param_names, &[program.type_name("Held")]);
            let unbox = member(
                schema.value_slots,
                ValueSymbol::declared("unbox", program.labels).unwrap(),
            )
            .expect("the signature declares `unbox`");
            let TypeNode::KFunction { params, .. } = program.types.node(unbox) else {
                panic!("the slot is a function type");
            };
            let x = BinderSymbol::classify("x").unwrap();
            let TypeNode::ConstructorApply { constructor, .. } =
                program.types.node(params.get(x.symbol()).expect("the parameter `x`"))
            else {
                panic!("the parameter applies the abstract member");
            };
            assert_eq!(constructor, boxed);
        },
    );
}

#[test]
fn a_declaration_the_door_cannot_elaborate_refuses_and_binds_nothing() {
    // Each case names the binder the refusal must leave claimed.
    for (source, left) in [
        // A cycle through a signature names no fresh identity, so it has no finite type. A
        // cycle through a transparent alias does not reach the door at all: a `LET`'s right-hand
        // side is eager, so the shape refuses it as an eager cycle first.
        ("SIG Holder = (VAL n :Nat)\nNEWTYPE Nat = :{s :Holder}", "Nat"),
        // A bare `TYPE` names an abstract member only a signature can bind.
        ("TYPE Loose", "Loose"),
        // A bodyless `GROUP` declares a chaining record, which operator groups owns.
        (
            "SIG Chained = ((TYPE Carrier) (GROUP FOLD LEFT = ((OP #(+) OVER Carrier))))",
            "Chained",
        ),
        // A repeated parameter name gives a family two slots under one label.
        ("NEWTYPE (Key Key AS Pair)", "Pair"),
        // An argument the family does not declare.
        (
            "NEWTYPE (Key AS Wrap)\nLET Bad = :(Wrap {Other = Number})",
            "Bad",
        ),
        // The arity-one sugar against a family of arity two.
        (
            "NEWTYPE (Key Val AS Pair)\nLET Bad = :(Number AS Pair)",
            "Bad",
        ),
        // A repeated tag gives a union two variants under one name.
        ("UNION Twice = (Tag :Number Tag :Str)", "Twice"),
        // A union with no variant declares nothing.
        ("UNION Empty = ()", "Empty"),
    ] {
        declared(source, |program, brought| {
            assert!(
                matches!(brought, Err(Elaboration::Unsupported { .. })),
                "`{source}` refuses: {brought:?}"
            );
            assert!(
                program.unbound(left),
                "`{source}` leaves `{left}` claimed by its binder"
            );
        });
    }
}

#[test]
fn a_projection_off_a_fellow_union_names_no_tag_yet() {
    declared(
        "UNION Maybe = (Some :Peek None :Null)\nNEWTYPE Peek = :{it :(Maybe.Some)}",
        |_, brought| {
            assert!(
                matches!(brought, Err(Elaboration::NoSuchMember { .. })),
                "the union declares no tag until it seals: {brought:?}"
            );
        },
    );
}

