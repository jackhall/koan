//! What a law cannot express.
//!
//! Each test here says which law would have covered it and why it cannot: a panic that has no
//! algebraic statement, a byte layout whose whole point is that it never moves, or a worked example
//! that pins the *edge* of a law rather than the law itself. Each builds its own registry over a
//! region of its own, which doubles as its scratch.

use crate::memory::{Bump, ScopeId};
use crate::symbols::{BinderSymbol, KeywordSymbol, SymbolInterner, TypeSymbol};

use crate::type_lattice::digest::{TypeDigest, empty_schema_digest, node_digest};
use crate::type_lattice::handle::KType;
use crate::type_lattice::kind::KKind;
use crate::type_lattice::lattice::{join, meet};
use crate::type_lattice::node::{NodeSchema, TypeNode};
use crate::type_lattice::order::is_subtype_of;
use crate::type_lattice::record::Record;
use crate::type_lattice::registry::TypeRegistry;
use crate::type_lattice::render::display_name;
use crate::type_lattice::schema::{SchemaDraft, shape_slots};
use crate::type_lattice::shape::{DeferredReturnSurface, DispatchTokenElement};
use crate::type_lattice::unify::{Collector, UnifyFailure, admits_with};
use crate::type_lattice::walk::Variance;

/// No law: a handle that names no interned node is a bug in whoever minted it, not a value the
/// algebra relates. The panic is the contract.
#[test]
#[should_panic(expected = "names no interned node")]
fn reading_an_uninterned_handle_panics() {
    let bump = Bump::new();
    let types = TypeRegistry::in_region(&bump);
    let stranger = KType::from_digest(TypeDigest(0xdead_beef));
    types.with_node(stranger, |_| ());
}

/// No law: `union_of` of nothing has no operand for a property to quantify over. The empty union is
/// the bottom, which is what makes `Never` the join's identity.
#[test]
fn a_union_of_nothing_is_never() {
    let bump = Bump::new();
    let region = &bump;
    let types = TypeRegistry::in_region(region);
    assert_eq!(types.union_of(region, &[]), KType::NEVER);
    assert_eq!(
        types.union_of(region, &[KType::NEVER, KType::NEVER]),
        KType::NEVER
    );
}

/// No law: the empty schema's byte layout is a fixed point of the recipe, and the whole value of
/// pinning it is that no property may derive it. `:Module` and a user's zero-member `SIG E = ()`
/// share one content identity.
#[test]
fn the_empty_schema_digest_is_the_module_top() {
    let bump = Bump::new();
    let region = &bump;
    let types = TypeRegistry::in_region(region);
    let empty = types.signature(region, SchemaDraft::new(region));
    assert_eq!(empty, KType::EMPTY_SIGNATURE);
    match types.node(empty) {
        TypeNode::Signature { schema_digest, .. } => {
            assert_eq!(schema_digest, empty_schema_digest());
        }
        _ => panic!("the signature door interned something else"),
    }
}

/// The **edge** of the solving law, not the law: the worked examples the design states for a
/// variable two slots reach from below.
#[test]
fn a_twice_used_variable_takes_the_join() {
    let symbols = SymbolInterner::new();
    let bump = Bump::new();
    let region = &bump;
    let types = TypeRegistry::in_region(region);
    let keyword = KeywordSymbol::declared("PURE", &symbols).expect("a keyword token");
    let element = types.quantified(0, KType::ANY);
    let name = TypeSymbol::declared("Elt", &symbols).expect("a Type token");
    let shape = types
        .shape_type(
            region,
            &[name],
            &[
                DispatchTokenElement::Keyword(keyword),
                DispatchTokenElement::Slot(element),
                DispatchTokenElement::Slot(element),
            ],
            &[],
            KType::NULL,
        )
        .handle;
    let slots: Vec<KType> = shape_slots(shape, &types).collect();
    let mixed = types.union_of(region, &[KType::NUMBER, KType::STR]);
    // Two arguments of one type solve to that type; any two others to their join, whether or not
    // either lies under the other.
    assert_eq!(
        solve_over(
            &types,
            region,
            &slots,
            &[KType::ANY],
            &[KType::NUMBER, KType::NUMBER]
        ),
        Ok(vec![KType::NUMBER])
    );
    assert_eq!(
        solve_over(
            &types,
            region,
            &slots,
            &[KType::ANY],
            &[KType::NUMBER, mixed]
        ),
        Ok(vec![mixed])
    );
    assert_eq!(
        solve_over(
            &types,
            region,
            &slots,
            &[KType::ANY],
            &[KType::NUMBER, KType::STR]
        ),
        Ok(vec![mixed])
    );
}

/// The edge of the solving law's dual: a variable two function-typed slots reach from above takes
/// the meet of what they contribute, which may be `Never`.
#[test]
fn a_variable_reached_from_above_takes_the_meet() {
    let symbols = SymbolInterner::new();
    let bump = Bump::new();
    let region = &bump;
    let types = TypeRegistry::in_region(region);
    let x = BinderSymbol::declared("x", &symbols).expect("a bindable token");
    let takes = |t: KType| {
        types
            .function_type(region, &[], &[(x, t)], KType::NULL)
            .handle
    };
    let position = takes(types.quantified(0, KType::ANY));
    let slots = [position, position];
    let solve = |a: KType, b: KType| {
        solve_over(&types, region, &slots, &[KType::ANY], &[takes(a), takes(b)])
    };
    assert_eq!(solve(KType::NUMBER, KType::STR), Ok(vec![KType::NEVER]));
    let number_or_str = types.union_of(region, &[KType::NUMBER, KType::STR]);
    let number_or_bool = types.union_of(region, &[KType::NUMBER, KType::BOOL]);
    assert_eq!(
        solve(number_or_str, number_or_bool),
        Ok(vec![KType::NUMBER])
    );
}

/// The edge of the failure rule: a solve fails only where its lower end lies above a ceiling, and
/// a contribution from above wider than the bound leaves the bound as the upper end.
#[test]
fn a_solve_fails_only_where_its_pair_denotes_nothing() {
    let symbols = SymbolInterner::new();
    let bump = Bump::new();
    let region = &bump;
    let types = TypeRegistry::in_region(region);
    let x = BinderSymbol::declared("x", &symbols).expect("a bindable token");
    let variable = types.quantified(0, KType::NUMBER);
    assert_eq!(
        solve_over(
            &types,
            region,
            &[variable, variable],
            &[KType::NUMBER],
            &[KType::NUMBER, KType::STR]
        ),
        Err(Some(0))
    );
    let takes = |t: KType| {
        types
            .function_type(region, &[], &[(x, t)], KType::NULL)
            .handle
    };
    assert_eq!(
        solve_over(
            &types,
            region,
            &[takes(variable)],
            &[KType::NUMBER],
            &[takes(KType::ANY)]
        ),
        Ok(vec![KType::NUMBER])
    );
}

/// The item's chain through a joined instance: a function generic over both its parameters lies
/// under its instance at `Number | Str`, which lies under a function taking a number and a string —
/// and the order reaches the last from the first in one step.
#[test]
fn the_order_closes_through_a_joined_instance() {
    let symbols = SymbolInterner::new();
    let bump = Bump::new();
    let region = &bump;
    let types = TypeRegistry::in_region(region);
    let x = BinderSymbol::declared("x", &symbols).expect("a bindable token");
    let y = BinderSymbol::declared("y", &symbols).expect("a bindable token");
    let elt = TypeSymbol::declared("Elt", &symbols).expect("a Type token");
    let takes = |group: &[TypeSymbol], first: KType, second: KType| {
        types
            .function_type(region, group, &[(x, first), (y, second)], KType::NULL)
            .handle
    };
    let variable = types.quantified(0, KType::ANY);
    let generic = takes(&[elt], variable, variable);
    let number_or_str = types.union_of(region, &[KType::NUMBER, KType::STR]);
    let joined = takes(&[], number_or_str, number_or_str);
    let split = takes(&[], KType::NUMBER, KType::STR);
    assert!(is_subtype_of(&types, region, generic, joined));
    assert!(is_subtype_of(&types, region, joined, split));
    assert!(is_subtype_of(&types, region, generic, split));
}

/// Admit each of `arguments` into its slot of `slots` through one collector bounded by `bounds`,
/// then solve. `Ok` carries the solution; `Err` carries the index of a variable whose pair denotes
/// nothing, or `None` for a mismatch.
fn solve_over(
    types: &TypeRegistry<'_>,
    region: &Bump,
    slots: &[KType],
    bounds: &[KType],
    arguments: &[KType],
) -> Result<Vec<KType>, Option<usize>> {
    let mut collector = Collector::new(region, bounds);
    for (slot, argument) in slots.iter().zip(arguments) {
        if admits_with(
            types,
            region,
            *slot,
            *argument,
            Variance::Co,
            &mut collector,
        )
        .is_err()
        {
            return Err(None);
        }
    }
    collector
        .solve(types)
        .map(|solution| solution.to_vec())
        .map_err(|failure| match failure {
            UnifyFailure::Disagree { index, .. } => Some(index),
            UnifyFailure::Mismatch => None,
        })
}

#[test]
fn a_single_occurrence_takes_its_bound_or_never() {
    let symbols = SymbolInterner::new();
    let bump = Bump::new();
    let region = &bump;
    let types = TypeRegistry::in_region(region);
    let keyword = KeywordSymbol::declared("PURE", &symbols).expect("a keyword token");
    let name = TypeSymbol::declared("Elt", &symbols).expect("a Type token");
    let variable = types.quantified(0, KType::NUMBER);
    let head = |slot: KType, ret: KType| {
        types
            .shape_type(
                region,
                &[name],
                &[
                    DispatchTokenElement::Keyword(keyword),
                    DispatchTokenElement::Slot(slot),
                ],
                &[],
                ret,
            )
            .handle
    };
    // Contravariant and alone: the caller picks it, so the slot accepts anything under the bound.
    let in_slot = head(variable, KType::NULL);
    assert_eq!(in_slot, head(KType::NUMBER, KType::NULL));
    // Covariant and alone: it must hold at every instantiation, which only the bottom does.
    let in_return = head(KType::STR, variable);
    assert_eq!(in_return, head(KType::STR, KType::NEVER));
}

/// No law: `Record`'s order-blind equality and the digest agreeing with it are properties of the
/// container, and the generated types never build two records differing only in field order.
#[test]
fn a_record_is_order_blind_in_identity_and_ordered_in_presentation() {
    let symbols = SymbolInterner::new();
    let bump = Bump::new();
    let region = &bump;
    let types = TypeRegistry::in_region(region);
    let x = BinderSymbol::declared("x", &symbols).expect("a bindable token");
    let y = BinderSymbol::declared("y", &symbols).expect("a bindable token");
    let forwards = [(x, KType::NUMBER), (y, KType::STR)];
    let backwards = [(y, KType::STR), (x, KType::NUMBER)];
    assert_eq!(Record::over(&forwards), Record::over(&backwards));
    let record = types.record(region, &forwards);
    assert_eq!(record, types.record(region, &backwards));
    let TypeNode::Record { fields } = types.node(record) else {
        panic!("the record door interned something else");
    };
    assert_eq!(
        fields.keys().collect::<Vec<_>>(),
        vec![x, y],
        "declaration order survives for rendering",
    );
}

/// The **edge** of the width laws: which side of a width verdict each arm sits on has no algebraic
/// statement beyond the order itself, so the four arms are pinned by example.
#[test]
fn width_runs_the_way_each_arm_declares() {
    let symbols = SymbolInterner::new();
    let bump = Bump::new();
    let region = &bump;
    let types = TypeRegistry::in_region(region);
    let x = BinderSymbol::declared("x", &symbols).expect("a bindable token");
    let y = BinderSymbol::declared("y", &symbols).expect("a bindable token");
    let wide = types.record(region, &[(x, KType::NUMBER), (y, KType::STR)]);
    let narrow = types.record(region, &[(x, KType::NUMBER)]);
    assert!(
        is_subtype_of(&types, region, wide, narrow),
        "records are width-superset"
    );
    assert!(!is_subtype_of(&types, region, narrow, wide));
    assert_eq!(meet(&types, region, wide, narrow), wide);
    assert_eq!(join(&types, region, wide, narrow), narrow);

    let few = types
        .function_type(region, &[], &[(x, KType::NUMBER)], KType::NULL)
        .handle;
    let many = types
        .function_type(
            region,
            &[],
            &[(x, KType::NUMBER), (y, KType::STR)],
            KType::NULL,
        )
        .handle;
    assert!(
        is_subtype_of(&types, region, few, many),
        "a function subtype asks for no name the supertype does not",
    );
    assert!(!is_subtype_of(&types, region, many, few));
}

/// No law: the canonical form of a quantified function is a claim about worked spellings — which
/// renamings and which written orders collapse to one handle, and what a lone occurrence becomes —
/// and a generated pair almost never spells two of them.
#[test]
fn a_quantified_function_interns_by_shape_whatever_its_names() {
    let symbols = SymbolInterner::new();
    let bump = Bump::new();
    let region = &bump;
    let types = TypeRegistry::in_region(region);
    let x = BinderSymbol::declared("x", &symbols).expect("a bindable token");
    let y = BinderSymbol::declared("y", &symbols).expect("a bindable token");
    let elt = TypeSymbol::declared("Elt", &symbols).expect("a Type token");
    let other = TypeSymbol::declared("Other", &symbols).expect("a Type token");
    let variable = types.quantified(0, KType::ANY);

    // `FN FOR ALL (Elt) :{x :Elt} -> Elt` under either spelling, and with the fields in either
    // written order, is one handle: the names are render-only and the record is order-blind.
    let identity = |group, param| {
        types
            .function_type(region, group, &[(x, param)], param)
            .handle
    };
    let (as_elt, as_other) = ([elt], [other]);
    assert_eq!(identity(&as_elt, variable), identity(&as_other, variable));
    let forward = types
        .function_type(region, &[elt], &[(x, variable), (y, KType::STR)], variable)
        .handle;
    let reversed = types
        .function_type(region, &[elt], &[(y, KType::STR), (x, variable)], variable)
        .handle;
    assert_eq!(forward, reversed);

    // A lone covariant occurrence is `Never`; a lone contravariant one is its bound.
    let lone_return = types
        .function_type(region, &[elt], &[(x, KType::NUMBER)], variable)
        .handle;
    assert_eq!(
        lone_return,
        types
            .function_type(region, &[], &[(x, KType::NUMBER)], KType::NEVER)
            .handle,
    );
    let lone_param = types
        .function_type(
            region,
            &[elt],
            &[(x, types.quantified(0, KType::NUMBER))],
            KType::STR,
        )
        .handle;
    assert_eq!(
        lone_param,
        types
            .function_type(region, &[], &[(x, KType::NUMBER)], KType::STR)
            .handle,
    );

    // The order instantiates: the quantified identity is below every monomorphic identity, and
    // below a quantified function that promises less about its return.
    let quantified_identity = identity(&as_elt, variable);
    let on_numbers = types
        .function_type(region, &[], &[(x, KType::NUMBER)], KType::NUMBER)
        .handle;
    assert!(is_subtype_of(
        &types,
        region,
        quantified_identity,
        on_numbers
    ));
    assert!(!is_subtype_of(
        &types,
        region,
        on_numbers,
        quantified_identity
    ));
    let to_any = types
        .function_type(region, &[elt], &[(x, variable)], KType::ANY)
        .handle;
    assert!(is_subtype_of(&types, region, quantified_identity, to_any));
}

/// No law: which family top a node kind lies under is a definition, not a property — the family
/// table in `order::family_top`. One representative per row pins it, with the rows whose family is
/// decided elsewhere: a type variable by its bound, a union by its members, and a deferred return
/// under no family at all.
#[test]
fn each_node_kind_lies_under_its_family_top() {
    let symbols = SymbolInterner::new();
    let x = BinderSymbol::declared("x", &symbols).expect("a bindable token");
    let name = TypeSymbol::declared("Elt", &symbols).expect("a Type token");
    let keyword = KeywordSymbol::declared("PURE", &symbols).expect("a keyword token");
    // Declared ahead of the registry, so it outlives the shape node interned over it.
    let elements = [DispatchTokenElement::Keyword(keyword)];
    let bump = Bump::new();
    let region = &bump;
    let types = TypeRegistry::in_region(region);
    let tops = [KType::ANY_VALUE, KType::ANY_TYPE, KType::ANY_CODE];
    let under = |ktype: KType| -> Vec<KType> {
        tops.into_iter()
            .filter(|top| is_subtype_of(&types, region, ktype, *top))
            .collect()
    };

    let values = [
        KType::NUMBER,
        KType::STR,
        KType::BOOL,
        KType::NULL,
        KType::LIST_OF_ANY,
        KType::DICT_ANY_ANY,
        types.record(region, &[(x, KType::ANY)]),
        types
            .function_type(region, &[], &[(x, KType::ANY)], KType::ANY)
            .handle,
        types.intern(
            region,
            TypeNode::ExpressionShape {
                quantifiers: &[],
                bounds: &[],
                elements: &elements,
                classes: &[],
                ret: KType::NUMBER,
            },
        ),
        types.constructor_apply(region, KType::NUMBER, &[(x, KType::ANY)]),
        KType::EMPTY_SIGNATURE,
        types.intern(
            region,
            TypeNode::SetMember {
                scc_digest: node_digest(region, &TypeNode::Number),
                index: 0,
                scc_size: 1,
                name,
                kind: KKind::NewType,
                schema: NodeSchema::NewType(KType::NUMBER),
            },
        ),
        types.sibling(0),
        KType::ANY_VALUE,
    ];
    for value in values {
        assert_eq!(under(value), [KType::ANY_VALUE], "{value:?}");
    }
    let codes = [
        KType::IDENTIFIER,
        KType::SYMBOL,
        KType::TYPE_NAME_TOKEN,
        KType::EXPRESSION,
        KType::SIGILED_TYPE_EXPR,
        KType::RECORD_TYPE,
        KType::LITERAL,
        KType::BLOCK,
        KType::DECLARATION,
        KType::BINDER,
        KType::NAME,
        KType::KEYWORD,
        KType::ANY_CODE,
    ];
    for code in codes {
        assert_eq!(under(code), [KType::ANY_CODE], "{code:?}");
    }
    for kind in [
        KKind::ProperType,
        KKind::Signature,
        KKind::AnyType,
        KKind::NewType,
        KKind::TypeConstructor,
    ] {
        assert_eq!(under(KType::of_kind(kind)), [KType::ANY_TYPE]);
    }

    // A variable answers by its bound; one bounded by `Any` — a sealed member's default — lies
    // under no family top, since the view hides which family it stands for.
    assert!(under(types.quantified(0, KType::ANY)).is_empty());
    assert_eq!(
        under(types.quantified(0, KType::ANY_VALUE)),
        [KType::ANY_VALUE]
    );
    let sealed = types.abstract_type(region, ScopeId::SENTINEL, name, &[], None, KType::ANY);
    assert!(under(sealed).is_empty());
    // A union answers by its members, and a deferred return, whose return is unknown, by none.
    let mixed = types.union_of(region, &[KType::NUMBER, KType::PROPER_TYPE]);
    assert!(under(mixed).is_empty());
    let pair_top = types.union_of(region, &[KType::ANY_VALUE, KType::ANY_TYPE]);
    assert!(is_subtype_of(&types, region, mixed, pair_top));
    assert!(under(types.deferred_return(DeferredReturnSurface::Type(name))).is_empty());
    assert!(under(KType::ANY).is_empty());

    // The families are disjoint and join to their union.
    assert_eq!(
        meet(&types, region, KType::ANY_VALUE, KType::ANY_CODE),
        KType::NEVER
    );
    assert_eq!(
        join(&types, region, KType::ANY_VALUE, KType::ANY_TYPE),
        pair_top
    );
}

/// The code family is a tree: each kind lies under its parent and, transitively, every kind above
/// it, and under nothing beside. The meets and joins that matter follow from the tree alone.
#[test]
fn the_code_kinds_form_a_tree_under_code() {
    let bump = Bump::new();
    let region = &bump;
    let types = TypeRegistry::in_region(region);
    let below = |a: KType, b: KType| is_subtype_of(&types, region, a, b);
    let edges = [
        (KType::BLOCK, KType::ANY_CODE),
        (KType::EXPRESSION, KType::BLOCK),
        (KType::DECLARATION, KType::EXPRESSION),
        (KType::LITERAL, KType::EXPRESSION),
        (KType::SYMBOL, KType::EXPRESSION),
        (KType::SIGILED_TYPE_EXPR, KType::EXPRESSION),
        (KType::RECORD_TYPE, KType::EXPRESSION),
        (KType::BINDER, KType::DECLARATION),
        (KType::NAME, KType::SYMBOL),
        (KType::KEYWORD, KType::SYMBOL),
        (KType::IDENTIFIER, KType::NAME),
        (KType::TYPE_NAME_TOKEN, KType::NAME),
    ];
    for (child, parent) in edges {
        assert!(below(child, parent), "{child:?} under {parent:?}");
        assert!(!below(parent, child), "{parent:?} not under {child:?}");
    }
    let chain = [
        KType::IDENTIFIER,
        KType::NAME,
        KType::SYMBOL,
        KType::EXPRESSION,
        KType::BLOCK,
        KType::ANY_CODE,
    ];
    for (index, lower) in chain.iter().enumerate() {
        for upper in &chain[index..] {
            assert!(below(*lower, *upper), "{lower:?} under {upper:?}");
        }
    }
    assert!(below(KType::BINDER, KType::EXPRESSION));
    for (a, b) in [
        (KType::KEYWORD, KType::NAME),
        (KType::LITERAL, KType::SYMBOL),
        (KType::BINDER, KType::SYMBOL),
        (KType::DECLARATION, KType::BINDER),
        (KType::BLOCK, KType::EXPRESSION),
        (KType::EXPRESSION, KType::SYMBOL),
        (KType::IDENTIFIER, KType::KEYWORD),
    ] {
        assert!(!below(a, b), "{a:?} not under {b:?}");
    }
    for kind in [KType::BLOCK, KType::NAME, KType::KEYWORD, KType::BINDER] {
        assert!(!below(kind, KType::ANY_VALUE), "{kind:?} not a value");
        assert!(!below(kind, KType::ANY_TYPE), "{kind:?} not a type");
    }

    assert_eq!(
        meet(&types, region, KType::EXPRESSION, KType::BLOCK),
        KType::EXPRESSION
    );
    assert_eq!(
        meet(&types, region, KType::NAME, KType::KEYWORD),
        KType::NEVER
    );
    assert_eq!(
        meet(&types, region, KType::LITERAL, KType::BINDER),
        KType::NEVER
    );
    assert_eq!(
        meet(&types, region, KType::DECLARATION, KType::SYMBOL),
        KType::NEVER
    );
    assert_eq!(
        types.union_of(region, &[KType::LITERAL, KType::EXPRESSION]),
        KType::EXPRESSION
    );

    let list_of_code = types.list(KType::ANY_CODE);
    assert!(below(KType::LIST_OF_NAME, list_of_code));
    assert!(below(list_of_code, KType::ANY_VALUE));
    assert!(below(
        KType::LIST_OF_DECLARATION,
        types.list(KType::EXPRESSION)
    ));
    assert!(below(
        KType::DICT_NAME_BLOCK,
        types.dict(KType::SYMBOL, KType::ANY_CODE)
    ));
    assert!(below(KType::TYPE_CODE, KType::EXPRESSION));
}

/// A code kind needing names lies under `Code`, over the same kind needing more and over a kind
/// under its own, and under the same kind needing fewer; the bare kind needs none. Two meet at
/// their kinds' meet needing the names both need, and one renders as its `NEEDING` spelling.
#[test]
fn a_code_kind_needing_names_is_ordered_by_kind_and_by_its_names() {
    let bump = Bump::new();
    let region = &bump;
    let types = TypeRegistry::in_region(region);
    let symbols = SymbolInterner::new();
    let name = |text: &str| BinderSymbol::declared(text, &symbols).expect("a bindable token");
    let (y, z) = (name("y"), name("z"));
    let below = |a: KType, b: KType| is_subtype_of(&types, region, a, b);
    let needing = |kind: KType, names: &[BinderSymbol]| types.code_needing(region, kind, names);

    assert_eq!(needing(KType::EXPRESSION, &[]), KType::EXPRESSION);
    assert_eq!(
        needing(KType::EXPRESSION, &[z, y, y]),
        needing(KType::EXPRESSION, &[y, z])
    );
    let expression_y = needing(KType::EXPRESSION, &[y]);
    let expression_yz = needing(KType::EXPRESSION, &[y, z]);
    let block_y = needing(KType::BLOCK, &[y]);
    let binder_y = needing(KType::BINDER, &[y]);
    let expression_z = needing(KType::EXPRESSION, &[z]);

    for (lower, upper) in [
        (KType::EXPRESSION, expression_y),
        (expression_y, expression_yz),
        (expression_y, block_y),
        (binder_y, expression_y),
        (KType::LITERAL, expression_y),
        (expression_yz, KType::ANY_CODE),
    ] {
        assert!(below(lower, upper), "{lower:?} under {upper:?}");
        assert!(!below(upper, lower), "{upper:?} not under {lower:?}");
    }
    for (a, b) in [
        (expression_y, KType::EXPRESSION),
        (expression_y, expression_z),
        (block_y, KType::EXPRESSION),
        (expression_y, KType::ANY_VALUE),
    ] {
        assert!(!below(a, b), "{a:?} not under {b:?}");
    }

    assert_eq!(meet(&types, region, expression_yz, block_y), expression_y);
    assert_eq!(
        meet(&types, region, expression_y, expression_z),
        KType::EXPRESSION
    );
    assert_eq!(
        meet(&types, region, block_y, KType::LITERAL),
        KType::LITERAL
    );
    assert_eq!(meet(&types, region, binder_y, KType::LITERAL), KType::NEVER);
    assert_eq!(
        join(&types, region, expression_y, expression_yz),
        expression_yz
    );

    assert_eq!(
        display_name(block_y, &types, &symbols).to_string(),
        ":(Block NEEDING #[y])"
    );
}

/// No law: the order's laws hold over union-bounded variables without naming one. These pin the
/// worked examples — a variable bounded by `Number | Str` against the unions above and around its
/// bound, in the order, a union's canonical form and the meet.
#[test]
fn a_union_bounded_variable_lies_under_every_union_above_its_bound() {
    let bump = Bump::new();
    let region = &bump;
    let types = TypeRegistry::in_region(region);
    let number_or_str = types.union_of(region, &[KType::NUMBER, KType::STR]);
    let elt = types.quantified(0, number_or_str);
    let wider = types.union_of(region, &[KType::NUMBER, KType::STR, KType::BOOL]);
    assert!(is_subtype_of(&types, region, elt, number_or_str));
    assert!(is_subtype_of(&types, region, elt, wider));
    assert!(!is_subtype_of(&types, region, elt, KType::NUMBER));

    // `Elt | Number | Str` holds nothing `Number | Str` does not.
    assert_eq!(
        types.union_of(region, &[elt, KType::NUMBER, KType::STR]),
        number_or_str
    );
    let elt_or_bool = types.union_of(region, &[elt, KType::BOOL]);
    assert_ne!(elt_or_bool, types.union_of(region, &[KType::BOOL]));

    // The meet keeps a variable whose bound spans the other side's members, from either side.
    assert_eq!(meet(&types, region, elt_or_bool, number_or_str), elt);
    assert_eq!(meet(&types, region, number_or_str, elt_or_bool), elt);
}

/// No law: the unifier's carried-variable rule is covered by a property, but the solution it
/// reaches is a worked example. `Y` bounded by `LIST OF Number` fills `LIST OF X`, solving `X` to
/// `Number` — and that closes the order's transitivity through a monomorphic instance.
#[test]
fn a_carried_variable_fills_what_its_bound_fills() {
    let symbols = SymbolInterner::new();
    let bump = Bump::new();
    let region = &bump;
    let types = TypeRegistry::in_region(region);
    let name = TypeSymbol::declared("Held", &symbols).expect("a Type token");
    let list_of_number = types.list(KType::NUMBER);
    let carried = types.abstract_type(region, ScopeId::SENTINEL, name, &[], None, list_of_number);
    let declared = types.list(types.quantified(0, KType::ANY));
    let mut collector = Collector::new(region, &[KType::ANY]);
    assert_eq!(
        admits_with(
            &types,
            region,
            declared,
            carried,
            Variance::Co,
            &mut collector
        ),
        Ok(())
    );
    assert_eq!(
        collector.solve(&types).map(|solution| solution.to_vec()),
        Ok(vec![KType::NUMBER])
    );

    // `FOR ALL (X) FN :{y :(LIST OF X) z :(LIST OF X)} -> Null` ≤ the `Number` instance ≤
    // `FOR ALL (Y UNDER :(LIST OF Number)) FN :{y :Y z :Y} -> Null`, and the first ≤ the third.
    let y = BinderSymbol::declared("y", &symbols).expect("a bindable token");
    let z = BinderSymbol::declared("z", &symbols).expect("a bindable token");
    let x_name = TypeSymbol::declared("Elt", &symbols).expect("a Type token");
    let each_list = types
        .function_type(
            region,
            &[x_name],
            &[(y, declared), (z, declared)],
            KType::NULL,
        )
        .handle;
    let on_numbers = types
        .function_type(
            region,
            &[],
            &[(y, list_of_number), (z, list_of_number)],
            KType::NULL,
        )
        .handle;
    let bounded = types.quantified(0, list_of_number);
    let each_bounded = types
        .function_type(region, &[name], &[(y, bounded), (z, bounded)], KType::NULL)
        .handle;
    // A bound spanning two declared members is admitted member by member: `Held` bounded by
    // `LIST OF Number | Str` fills `LIST OF X | Str`, though neither member alone takes it.
    let spanning_bound = types.union_of(region, &[list_of_number, KType::STR]);
    let spanning = types.abstract_type(region, ScopeId::SENTINEL, name, &[], None, spanning_bound);
    let either = types.union_of(region, &[declared, KType::STR]);
    let mut collector = Collector::new(region, &[KType::ANY]);
    assert_eq!(
        admits_with(
            &types,
            region,
            either,
            spanning,
            Variance::Co,
            &mut collector
        ),
        Ok(())
    );
    assert_eq!(
        collector.solve(&types).map(|solution| solution.to_vec()),
        Ok(vec![KType::NUMBER])
    );

    assert!(is_subtype_of(&types, region, each_list, on_numbers));
    assert!(is_subtype_of(&types, region, on_numbers, each_bounded));
    assert!(is_subtype_of(&types, region, each_list, each_bounded));
}

/// No law: the regression the solution law's sampler found. A carried deferred return lies under
/// nothing but itself and `Any`, so a slot that mentions a variable admits it no more than the
/// same slot closed does — else the solved slot rejects the argument its admission let through.
#[test]
fn a_carried_deferred_return_fills_no_slot_it_is_not_under() {
    let symbols = SymbolInterner::new();
    let bump = Bump::new();
    let region = &bump;
    let types = TypeRegistry::in_region(region);
    let name = TypeSymbol::declared("Elt", &symbols).expect("a Type token");
    let deferred = types.deferred_return(DeferredReturnSurface::Type(name));
    let admits = |declared: KType| {
        let mut collector = Collector::new(region, &[KType::ANY]);
        admits_with(
            &types,
            region,
            declared,
            deferred,
            Variance::Co,
            &mut collector,
        )
    };
    let variable = types.quantified(0, KType::ANY);
    assert_eq!(admits(types.list(variable)), Err(UnifyFailure::Mismatch));
    assert_eq!(admits(types.list(KType::ANY)), Err(UnifyFailure::Mismatch));
    assert_eq!(admits(variable), Ok(()));
}

/// No law: a spelling. A group with a bounded quantifier renders as the dict it is written as, each
/// name beside its bound, `Any` included.
#[test]
fn a_bounded_quantifier_renders_under_its_bound() {
    let symbols = SymbolInterner::new();
    let bump = Bump::new();
    let region = &bump;
    let types = TypeRegistry::in_region(region);
    let a = BinderSymbol::declared("a", &symbols).expect("a bindable token");
    let b = BinderSymbol::declared("b", &symbols).expect("a bindable token");
    let elt = TypeSymbol::declared("Elt", &symbols).expect("a Type token");
    let key = TypeSymbol::declared("Key", &symbols).expect("a Type token");
    let value_bounded = types.quantified(0, KType::ANY_VALUE);
    let free = types.quantified(1, KType::ANY);
    let pair = types
        .function_type(
            region,
            &[elt, key],
            &[(a, value_bounded), (b, free)],
            types.dict(value_bounded, free),
        )
        .handle;
    // Canonical form orders the group by first occurrence, which puts `Key` first here.
    assert_eq!(
        crate::type_lattice::display_name(pair, &types, &symbols).to_string(),
        ":(FN FOR ALL #{Key: Any, Elt: Value} :{a :Elt b :Key} -> :(MAP Elt -> Key))"
    );
    let alone = types
        .function_type(region, &[elt], &[(a, value_bounded)], value_bounded)
        .handle;
    let rendered = crate::type_lattice::display_name(alone, &types, &symbols).to_string();
    assert!(rendered.contains("FOR ALL #{Elt: Value} "), "{rendered}");
    let number_or_str = types.union_of(region, &[KType::NUMBER, KType::STR]);
    let union_bounded = types.quantified(0, number_or_str);
    let spanning = types
        .function_type(region, &[elt], &[(a, union_bounded)], union_bounded)
        .handle;
    let rendered = crate::type_lattice::display_name(spanning, &types, &symbols).to_string();
    assert!(
        rendered.contains("FOR ALL #{Elt: :(Number | Str)} "),
        "{rendered}"
    );
}

/// No law: the property holds `bound_above` over every instance, but not how tight it is. These pin
/// the worked examples — each free variable at its bound where it is produced and at `Never` where
/// it is taken, a variable its own binder holds left alone, and a closed type unchanged.
#[test]
fn bounding_above_reads_a_free_variable_by_its_position() {
    let symbols = SymbolInterner::new();
    let bump = Bump::new();
    let region = &bump;
    let types = TypeRegistry::in_region(region);
    let above = |kt| crate::type_lattice::bound_above(&types, region, kt);
    let y = BinderSymbol::declared("y", &symbols).expect("a bindable token");
    let tee = TypeSymbol::declared("Tee", &symbols).expect("a Type token");
    let elt = types.quantified(0, KType::ANY);

    assert_eq!(above(types.list(elt)), types.list(KType::ANY));
    let taking = types.function_type(region, &[], &[(y, elt)], elt).handle;
    let widest = types
        .function_type(region, &[], &[(y, KType::NEVER)], KType::ANY)
        .handle;
    assert_eq!(above(taking), widest);
    let number_or_str = types.union_of(region, &[KType::NUMBER, KType::STR]);
    assert_eq!(above(types.quantified(0, number_or_str)), number_or_str);

    let closed = types.function_type(region, &[tee], &[(y, elt)], elt).handle;
    assert_eq!(above(closed), closed);
    let list_of_number = types.list(KType::NUMBER);
    assert_eq!(above(list_of_number), list_of_number);
}
