//! The door a component of type binders comes into being through: each declaration it elaborates,
//! each group it seals, and each refusal.

use crate::memory::BumpAllocator;
use crate::symbols::{BinderSymbol, KeywordSymbol, SymbolInterner, ValueSymbol};
use crate::type_lattice::{
    FoldDirection, KKind, KType, NodeSchema, ReductionMode, SigOrigin, TypeNode, TypeRegistry,
    display_name, member, shape_return, shape_slots,
};

use super::{Held, Program, brought, declared, with_program};
use crate::scope::Elaboration;

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
    brought("UNION Maybe = #{Some: Number, None: Null}", |program| {
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
    brought(
        "LET Alias = Number\nSIG HasLabel = #[(VAL label :Str)]",
        |program| {
            assert_eq!(program.bound("Alias"), KType::NUMBER);
            let TypeNode::Signature { schema, .. } = program.types.node(program.bound("HasLabel"))
            else {
                panic!("a SIG binds a signature");
            };
            let label = ValueSymbol::declared("label", program.symbols).unwrap();
            assert_eq!(member(schema.value_slots, label), Some(KType::STR));
        },
    );
}

#[test]
fn two_declarations_of_the_same_shape_intern_equal() {
    // Identity is content, so the same source in two programs — and a ring written in either
    // order — reaches one handle.
    let sig = |source| brought(source, |program| program.bound("HasLabel"));
    assert_eq!(
        sig("SIG HasLabel = #[(VAL label :Str)]"),
        sig("SIG HasLabel = #[(VAL label :Str)]")
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
        assert_eq!(
            fields.get(next.symbol()),
            Some(ring),
            "the field names the member"
        );
    });
    brought("UNION Tree = #{Node: Tree, Leaf: Null}", |program| {
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
        "UNION Nat = #{Zero: Null, Succ: Nat}\nNEWTYPE Wrap = :{n :Nat}",
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
fn a_signature_declares_its_parameters_and_manifest_members() {
    brought(
        "SIG Fixed FOR ALL #[Carrier] = #[(LET Elem = Number) (VAL x :Elem) (VAL c :Carrier)]",
        |program| {
            let TypeNode::Signature { schema, .. } = program.types.node(program.bound("Fixed"))
            else {
                panic!("a SIG binds a signature");
            };
            assert_eq!(schema.origin, SigOrigin::Declared);
            let carrier = program.type_name("Carrier");
            let rigid =
                member(schema.parameters, carrier).expect("the signature declares `Carrier`");
            assert!(
                matches!(
                    program.types.node(rigid),
                    TypeNode::Parameter {
                        bound: KType::ANY,
                        nonce: None,
                        ..
                    }
                ),
                "an unbounded head parameter is bounded by `Any`"
            );
            assert_eq!(
                member(schema.manifest_members, program.type_name("Elem")),
                Some(KType::NUMBER),
                "a manifest member fixes its type"
            );
            let slot = |name| {
                member(
                    schema.value_slots,
                    ValueSymbol::declared(name, program.symbols).unwrap(),
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
        "SIG Ring FOR ALL #[Carrier] = \
         #[(OP #(+) OVER Carrier) (UNARY OP #(~) OVER Carrier -> Carrier)]",
        |program| {
            let TypeNode::Signature { schema, .. } = program.types.node(program.bound("Ring"))
            else {
                panic!("a SIG binds a signature");
            };
            assert_eq!(schema.keyworded.len(), 2);
            assert!(
                schema.operators.is_empty(),
                "a head outside a `GROUP` declares no chaining"
            );
        },
    );
}

#[test]
fn a_bodyless_head_spells_the_shape_its_definition_spells() {
    // A definition registers the shape built from its function type over its head, which must be
    // the one its bodyless `SIG` twin spells. The operator twins share one builder. `pair` and
    // `swap` write the same four names in swapped roles, so whichever name sorts first by digest,
    // one of them has a first-written slot that is not the first sorted parameter: a shape that
    // copied the function type's numbering fails on it. `first` drops a variable that occurs once,
    // and `least` carries a bound.
    brought(
        "SIG Arith = #[(OP #(+) OVER Number) \
                      (UNARY OP #(~) OVER Number -> Number) \
                      (EXPR #(TWICE _ :Number) -> Number) \
                      (EXPR FOR ALL #[Elt Key] #(PAIR _ :Elt _ :Key _ :Elt _ :Key) -> Elt) \
                      (EXPR FOR ALL #[Elt Key] #(SWAP _ :Elt _ :Key _ :Elt _ :Key) -> Elt) \
                      (EXPR FOR ALL #[Elt] #(FIRST _ :Elt _ :Number) -> Number) \
                      (EXPR FOR ALL #{Elt: Number} #(LEAST _ :Elt _ :Elt) -> Elt)]\n\
         LET plus = OP #(+) OVER Number = #(left)\n\
         LET negate = UNARY OP #(~) OVER Number -> Number = #(operands)\n\
         LET twice = FN EXPR #(TWICE x :Number) -> Number = #(x)\n\
         LET pair = FN EXPR FOR ALL #[Elt Key] #(PAIR p :Elt q :Key r :Elt s :Key) -> Elt = #(p)\n\
         LET swap = FN EXPR FOR ALL #[Elt Key] #(SWAP q :Elt p :Key s :Elt r :Key) -> Elt = #(q)\n\
         LET first = FN EXPR FOR ALL #[Elt] #(FIRST x :Elt n :Number) -> Number = #(n)\n\
         LET least = FN EXPR FOR ALL #{Elt: Number} #(LEAST x :Elt y :Elt) -> Elt = #(x)",
        |program| {
            let TypeNode::Signature { schema, .. } = program.types.node(program.bound("Arith"))
            else {
                panic!("a SIG binds a signature");
            };
            let defined = |name| {
                program
                    .callable(name, true)
                    .expect("the definition elaborates")
                    .registered
                    .expect("a registration")
                    .shape
            };
            let mut declared: Vec<KType> = schema.keyworded.to_vec();
            let mut satisfiers: Vec<KType> =
                ["plus", "negate", "twice", "pair", "swap", "first", "least"]
                    .map(defined)
                    .to_vec();
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
fn a_declaration_the_door_cannot_elaborate_refuses_and_binds_nothing() {
    // Each case names the binder whose slot the refusal must leave empty.
    for (source, left) in [
        // A cycle through a signature names no fresh identity, so it has no finite type. A
        // cycle through a transparent alias does not reach the door at all: a `LET`'s right-hand
        // side is eager, so the shape refuses it as an eager cycle first.
        (
            "SIG Holder = #[(VAL n :Nat)]\nNEWTYPE Nat = :{s :Holder}",
            "Nat",
        ),
        // `{+}` alone is not the builtin additive group, so it would chain `+` a second way.
        (
            "SIG Chained FOR ALL #[Carrier] = #[(GROUP FOLD LEFT = #[(OP #(+) OVER Carrier)])]",
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
        ("UNION Twice = #{Tag: Number, Tag: Str}", "Twice"),
    ] {
        declared(source, |program, brought| {
            assert!(
                matches!(brought, Err(Elaboration::Unsupported { .. })),
                "`{source}` refuses: {brought:?}"
            );
            assert!(program.unbound(left), "`{source}` leaves `{left}` empty");
        });
    }
}

/// `Kind NEEDING #[…]` is the code kind needing the names its one-name quotes and the bucket keys
/// its keyword quotes spell, in any order; a kind that is no code, or a list element that is
/// neither, refuses.
#[test]
fn a_code_kind_needing_names_is_spelled_with_needing() {
    fn expression(
        _: &TypeRegistry<'_>,
        _: BumpAllocator<'_>,
        _: &SymbolInterner,
    ) -> Vec<(&'static str, KType)> {
        vec![("Expression", KType::EXPRESSION)]
    }
    let declared = |source: &str, check: &dyn Fn(Program<'_, '_, '_>, Result<(), Elaboration>)| {
        with_program(
            source,
            expression,
            |_, _, _| Held::Empty,
            |program| {
                let brought = program.declare();
                check(program, brought)
            },
        )
    };
    declared(
        "LET Needs = :(Expression NEEDING #[y it Carrier])\nLET Same = :(Expression NEEDING #[Carrier it y y])",
        &|program, brought| {
            brought.expect("a code kind needing names declares");
            let names = ["y", "it", "Carrier"].map(|text| BinderSymbol::classify(text).unwrap());
            let needing = program
                .types
                .code_needing(program.scratch, KType::EXPRESSION, &names);
            assert_eq!(program.bound("Needs"), needing);
            assert_eq!(program.bound("Same"), needing);
        },
    );
    declared(
        "LET Keyed = :(Expression NEEDING #[(LOG _) y])",
        &|program, brought| {
            brought.expect("a code kind needing a key declares");
            let log = BinderSymbol::Key(program.symbols.key("LOG _").unwrap());
            let y = BinderSymbol::classify("y").unwrap();
            let needing = program
                .types
                .code_needing(program.scratch, KType::EXPRESSION, &[log, y]);
            assert_eq!(program.bound("Keyed"), needing);
            let rendered = display_name(needing, program.types, program.symbols).to_string();
            assert!(rendered.contains("#[(LOG _) y]") || rendered.contains("#[y (LOG _)]"));
        },
    );
    for source in [
        "LET Bad = :(Number NEEDING #[y])",
        "LET Bad = :(Expression NEEDING #[(a b)])",
        "LET Bad = :(Expression NEEDING [1])",
    ] {
        declared(source, &|program, brought| {
            assert!(
                matches!(brought, Err(Elaboration::Unsupported { .. })),
                "`{source}` refuses: {brought:?}"
            );
            assert!(program.unbound("Bad"), "`{source}` leaves `Bad` empty");
        });
    }
}

#[test]
fn a_projection_off_a_fellow_union_names_no_tag_yet() {
    declared(
        "UNION Maybe = #{Some: Peek, None: Null}\nNEWTYPE Peek = :{it :(Maybe.Some)}",
        |_, brought| {
            assert!(
                matches!(brought, Err(Elaboration::NoSuchMember { .. })),
                "the union declares no tag until it seals: {brought:?}"
            );
        },
    );
}

/// The chaining records `name`'s signature declares, as (members, mode) pairs.
fn operators(
    program: &Program<'_, '_, '_>,
    name: &str,
) -> Vec<(Vec<KeywordSymbol>, ReductionMode)> {
    let TypeNode::Signature { schema, .. } = program.types.node(program.bound(name)) else {
        panic!("a SIG binds a signature");
    };
    schema
        .operators
        .iter()
        .map(|group| (group.members.to_vec(), group.mode))
        .collect()
}

fn operator(text: &str, program: &Program<'_, '_, '_>) -> KeywordSymbol {
    KeywordSymbol::declared(text, program.symbols).expect("a keyword token")
}

#[test]
fn a_bodyless_group_head_declares_a_chaining_record_over_its_heads() {
    brought(
        "SIG Ring FOR ALL #[Carrier] = #[\
         (GROUP FOLD RIGHT = #[(OP #(@) OVER Carrier) (OP #(%) OVER Carrier)])]",
        |program| {
            let mut members = vec![operator("@", &program), operator("%", &program)];
            members.sort_unstable();
            assert_eq!(
                operators(&program, "Ring"),
                [(members, ReductionMode::FoldRight)]
            );
            let TypeNode::Signature { schema, .. } = program.types.node(program.bound("Ring"))
            else {
                panic!("a SIG binds a signature");
            };
            assert_eq!(
                schema.keyworded.len(),
                2,
                "a group's heads are keyworded members like any other"
            );
        },
    );
    // A pairwise group takes its combiner from the quote, and its members may state a result.
    brought(
        "SIG Cmp FOR ALL #[Carrier] = #[\
         (GROUP PAIRWISE FOLD #(AND) LEFT = #[(OP #(~) OVER Carrier -> Bool)])]",
        |program| {
            assert_eq!(
                operators(&program, "Cmp"),
                [(
                    vec![operator("~", &program)],
                    ReductionMode::Pairwise {
                        combiner: operator("AND", &program),
                        direction: FoldDirection::Left,
                    }
                )]
            );
        },
    );
    // A group written out equal to a builtin one says what the language already says, and is one
    // signature member like any other.
    brought(
        "SIG Sum = #[(GROUP FOLD LEFT = #[(OP #(+) OVER Number) (OP #(-) OVER Number)])]",
        |program| {
            assert_eq!(operators(&program, "Sum").len(), 1);
        },
    );
}

#[test]
fn two_signatures_differing_only_in_a_groups_direction_are_two_handles() {
    let sig = |mode: &str| {
        brought(
            &format!(
                "SIG Ring = #[(GROUP {mode} = #[(OP #(@) OVER Number) (OP #(%) OVER Number)])]"
            ),
            |program| program.bound("Ring"),
        )
    };
    assert_eq!(sig("FOLD LEFT"), sig("FOLD LEFT"), "identity is content");
    assert_ne!(
        sig("FOLD LEFT"),
        sig("FOLD RIGHT"),
        "how a run reduces is part of what the signature says"
    );
}

#[test]
fn a_group_head_the_door_cannot_read_refuses() {
    // A group head whose body is not a run of binary operator declarations — a `UNARY OP` among
    // them, or no operator at all — never reaches the door: the shape's own member scan refuses it.
    for source in [
        // One symbol, two chainings.
        "SIG Bad = #[(GROUP FOLD LEFT = #[(OP #(@) OVER Number)]) \
         (GROUP FOLD RIGHT = #[(OP #(@) OVER Number)])]",
        // A fold carries its operand type forward, so a member of one states no result.
        "SIG Bad = #[(GROUP FOLD LEFT = #[(OP #(@) OVER Number -> Bool)])]",
        // `==` and `!=` belong to no group.
        "SIG Bad = #[(GROUP FOLD LEFT = #[(OP #(==) OVER Number -> Bool)])]",
        // A result of its own, on a symbol that folds.
        "SIG Bad = #[(OP #(@) OVER Number -> Bool)]",
        // A user's `==` answers `Bool`, since `!=` is its negation by construction.
        "SIG Bad = #[(OP #(==) OVER Number)]",
    ] {
        declared(source, |program, brought| {
            assert!(
                matches!(brought, Err(Elaboration::Unsupported { .. })),
                "`{source}` refuses: {brought:?}"
            );
            assert!(program.unbound("Bad"), "`{source}` leaves `Bad` empty");
        });
    }
    // The same returning head over a symbol that chains pairwise is admitted.
    brought(
        "SIG Cmp = #[(OP #(<) OVER Number -> Bool) (OP #(==) OVER Number -> Bool)]",
        |program| {
            let TypeNode::Signature { schema, .. } = program.types.node(program.bound("Cmp"))
            else {
                panic!("a SIG binds a signature");
            };
            assert_eq!(schema.keyworded.len(), 2);
        },
    );
}

/// The bound of the head parameter `name` of the signature bound to `sig`.
fn member_bound(program: &Program<'_, '_, '_>, sig: &str, name: &str) -> KType {
    let TypeNode::Signature { schema, .. } = program.types.node(program.bound(sig)) else {
        panic!("a SIG binds a signature");
    };
    let rigid = member(schema.parameters, program.type_name(name))
        .expect("the signature declares the parameter");
    let TypeNode::Parameter { bound, .. } = program.types.node(rigid) else {
        panic!("a head parameter is a rigid variable");
    };
    bound
}

#[test]
fn a_bounded_head_parameter_carries_its_bound() {
    let source = "\
SIG Plain FOR ALL #{Carrier: Number} = #[(VAL c :Carrier)]
SIG Either FOR ALL #{Carrier: :(Number | Str)} = #[(VAL c :Carrier)]
LET Base = Number
SIG Based FOR ALL #{Carrier: Base} = #[(VAL c :Carrier)]";
    brought(source, |program| {
        let (types, scratch) = (program.types, program.scratch);
        assert_eq!(member_bound(&program, "Plain", "Carrier"), KType::NUMBER);
        assert_eq!(
            member_bound(&program, "Either", "Carrier"),
            types.union_of(scratch, &[KType::NUMBER, KType::STR])
        );
        assert_eq!(
            member_bound(&program, "Based", "Carrier"),
            KType::NUMBER,
            "an alias reads as its type"
        );
    });
}

#[test]
fn a_bound_naming_a_parameter_or_on_a_family_is_refused() {
    declared(
        "SIG Chained FOR ALL #{Key: Any, Elt: Key} = #[(VAL e :Elt)]",
        |_, brought| assert!(matches!(brought, Err(Elaboration::Bound { .. }))),
    );
    declared("NEWTYPE (Elt UNDER Number)", |_, brought| {
        assert!(matches!(brought, Err(Elaboration::Unsupported { .. })))
    });
}

/// A signature elaborates a member of every shape the table says declares one, and a `LET` of a
/// type name besides: `declares_member` and the signature door name one set.
#[test]
fn every_member_declaring_shape_is_a_signature_member() {
    use crate::parse::builtin_shapes::{BUILTIN_SHAPES, BuiltinShapeId};
    let members = [
        (BuiltinShapeId::Val, "VAL x :Str"),
        (
            BuiltinShapeId::ExpressionHead,
            "EXPR #(FOO _ :Number) -> Number",
        ),
        (
            BuiltinShapeId::QuantifiedExpressionHead,
            "EXPR FOR ALL #[Elt] #(FOO _ :Elt) -> Elt",
        ),
        (BuiltinShapeId::OperatorHead, "OP #(@) OVER Number"),
        (
            BuiltinShapeId::OperatorHeadReturning,
            "OP #(==) OVER Number -> Bool",
        ),
        (
            BuiltinShapeId::UnaryOperatorHeadReturning,
            "UNARY OP #(~) OVER Number -> Number",
        ),
        (
            BuiltinShapeId::GroupHeadFoldLeft,
            "GROUP FOLD LEFT = #[(OP #(@) OVER Number)]",
        ),
        (
            BuiltinShapeId::GroupHeadFoldRight,
            "GROUP FOLD RIGHT = #[(OP #(@) OVER Number)]",
        ),
        (
            BuiltinShapeId::GroupHeadPairwiseFoldLeft,
            "GROUP PAIRWISE FOLD #(AND) LEFT = #[(OP #(~) OVER Number -> Bool)]",
        ),
        (
            BuiltinShapeId::GroupHeadPairwiseFoldRight,
            "GROUP PAIRWISE FOLD #(AND) RIGHT = #[(OP #(~) OVER Number -> Bool)]",
        ),
    ];
    let declaring: Vec<BuiltinShapeId> = BUILTIN_SHAPES
        .iter()
        .filter(|shape| shape.id.declares_member() && !shape.reserved)
        .map(|shape| shape.id)
        .collect();
    let listed: Vec<BuiltinShapeId> = members.iter().map(|(id, _)| *id).collect();
    assert_eq!(listed, declaring, "every member-declaring shape is listed");
    for (_, member) in members
        .iter()
        .chain([&(BuiltinShapeId::LetValue, "LET Elem = Number")])
    {
        brought(&format!("SIG Sg = #[({member})]"), |_| {});
    }
}

#[test]
fn a_meet_of_signatures_holds_both() {
    brought(
        "SIG Ranked = #[(EXPR #(MOVE 2 :Number TO 1 :Str) -> Number)]\n\
         SIG Written = #[(EXPR #(MOVE _ :Number TO _ :Str) -> Number)]\n\
         LET Both = :(Ranked & Written)",
        |program| {
            let TypeNode::SignatureMeet { members } = program.types.node(program.bound("Both"))
            else {
                panic!("two unordered signatures meet at the set of both");
            };
            let mut expected = [program.bound("Ranked"), program.bound("Written")];
            expected.sort_unstable();
            assert_eq!(members, expected);
        },
    );
}

#[test]
fn a_signatures_members_read_its_head_parameters() {
    brought(
        "SIG Stack FOR ALL #{Elt: Any} = #[(EXPR #(PUSH _ :Elt) -> :(LIST OF Elt))]",
        |program| {
            let TypeNode::Signature { schema, .. } = program.types.node(program.bound("Stack"))
            else {
                panic!("a SIG binds a signature");
            };
            let elt = member(schema.parameters, program.type_name("Elt")).expect("`Elt`");
            let [push] = schema.keyworded else {
                panic!("one keyworded member");
            };
            let slots: Vec<KType> = shape_slots(*push, program.types).collect();
            assert_eq!(slots, [elt]);
            assert_eq!(
                shape_return(*push, program.types),
                Some(program.types.list(elt))
            );
        },
    );
}

#[test]
fn a_member_may_quantify_its_own_variables() {
    // A keyworded member's own group, a `VAL` typed by a quantified function type, and a member
    // reading a head parameter under its own group.
    brought(
        "SIG Boxes = #[(EXPR FOR ALL #[Elt] #(BOX _ :Elt) -> :(LIST OF Elt))]\n\
         SIG Ident = #[(VAL identity :(FN FOR ALL #[Item] :{x :Item} -> Item))]\n\
         SIG Mappable FOR ALL #[Elt] = #[(EXPR FOR ALL #[Next] \
           #(MAP _ :(LIST OF Elt) WITH _ :(FN :{x :Elt} -> Next)) -> :(LIST OF Next))]",
        |program| {
            for name in ["Boxes", "Ident", "Mappable"] {
                assert!(
                    matches!(
                        program.types.node(program.bound(name)),
                        TypeNode::Signature { .. }
                    ),
                    "`{name}` elaborates"
                );
            }
        },
    );
}

#[test]
fn a_repeated_head_parameter_is_refused() {
    declared(
        "SIG Twice FOR ALL #[Elt Elt] = #[(VAL e :Elt)]",
        |_, brought| {
            assert!(brought.is_err(), "{brought:?}");
        },
    );
}
