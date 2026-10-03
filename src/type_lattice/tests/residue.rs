//! What a law cannot express.
//!
//! Each test here says which law would have covered it and why it cannot: a panic that has no
//! algebraic statement, a byte layout whose whole point is that it never moves, or a worked example
//! that pins the *edge* of a law rather than the law itself. Each builds its own registry over a
//! region of its own, which doubles as its scratch.

use crate::memory::Bump;
use crate::symbols::{BinderSymbol, KeywordSymbol, SymbolInterner, TypeSymbol};

use crate::type_lattice::digest::{TypeDigest, node_digest, schema_content_digest};
use crate::type_lattice::handle::{Handle, KType, Parametric, TypeHandle};
use crate::type_lattice::kind::KKind;
use crate::type_lattice::lattice::{join, meet_through_variables as meet};
use crate::type_lattice::node::{NodeSchema, TypeNode};
use crate::type_lattice::order::{fits, is_subtype_of};
use crate::type_lattice::record::Record;
use crate::type_lattice::registry::TypeRegistry;
use crate::type_lattice::render::display_handle;
use crate::type_lattice::run::Elements;
use crate::type_lattice::schema::{SchemaDraft, SigOrigin, SigSchema};
use crate::type_lattice::shape::shape_slots;
use crate::type_lattice::shape::{DeferredReturnSurface, DispatchTokenElement};
use crate::type_lattice::typed;
use crate::type_lattice::unify::{Collector, UnifyFailure, admits};
use crate::type_lattice::verdicts::Relation;
use crate::type_lattice::walk::Variance;

/// No law: a handle that names no interned node is a bug in whoever minted it, not a value the
/// algebra relates. The panic is the contract.
#[test]
#[should_panic(expected = "names no interned node")]
fn reading_an_uninterned_handle_panics() {
    let bump = Bump::new();
    let types = TypeRegistry::in_region(&bump);
    let stranger = Handle::from_digest(TypeDigest(0xdead_beef));
    types.node(stranger);
}

/// No law: `union_of` of nothing has no operand for a property to quantify over. The empty union is
/// the bottom, which is what makes `Never` the join's identity.
#[test]
fn a_union_of_nothing_is_never() {
    let bump = Bump::new();
    let region = &bump;
    let types = TypeRegistry::in_region(region);
    assert_eq!(types.union_of::<Handle>(region, &[]), Handle::NEVER);
    assert_eq!(
        types.union_of(region, &[Handle::NEVER, Handle::NEVER]),
        Handle::NEVER
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
            assert_eq!(schema_digest, schema_content_digest(SigSchema::EMPTY));
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
    let element = types.quantified(0, KType::ANY).raw();
    let name = TypeSymbol::declared("Elt", &symbols).expect("a Type token");
    let shape = types
        .shape_group(
            region,
            &[name],
            &[KType::ANY],
            &[
                DispatchTokenElement::Keyword(keyword),
                DispatchTokenElement::Slot(element),
                DispatchTokenElement::Slot(element),
            ],
            &[],
            KType::NULL.raw(),
        )
        .0;
    let slots: Vec<Handle> = shape_slots(shape, &types).collect();
    let mixed = types.union_of(region, &[KType::NUMBER.raw(), KType::STR.raw()]);
    // Two arguments of one type solve to that type; any two others to their join, whether or not
    // either lies under the other.
    assert_eq!(
        solve_over(
            &types,
            region,
            &slots,
            &[KType::ANY],
            &[KType::NUMBER.raw(), KType::NUMBER.raw()]
        ),
        Ok(vec![KType::NUMBER.raw()])
    );
    assert_eq!(
        solve_over(
            &types,
            region,
            &slots,
            &[KType::ANY],
            &[KType::NUMBER.raw(), mixed]
        ),
        Ok(vec![mixed])
    );
    assert_eq!(
        solve_over(
            &types,
            region,
            &slots,
            &[KType::ANY],
            &[KType::NUMBER.raw(), KType::STR.raw()]
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
    let takes = |t: Handle| types.function_type(region, &[(x, t)], KType::NULL.raw());
    let position = takes(types.quantified(0, KType::ANY).raw());
    let slots = [position, position];
    let solve = |a: Handle, b: Handle| {
        solve_over(&types, region, &slots, &[KType::ANY], &[takes(a), takes(b)])
    };
    assert_eq!(
        solve(KType::NUMBER.raw(), KType::STR.raw()),
        Ok(vec![Handle::NEVER])
    );
    let number_or_str = types.union_of(region, &[KType::NUMBER.raw(), KType::STR.raw()]);
    let number_or_bool = types.union_of(region, &[KType::NUMBER.raw(), KType::BOOL.raw()]);
    assert_eq!(
        solve(number_or_str, number_or_bool),
        Ok(vec![KType::NUMBER.raw()])
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
    let variable = types.quantified(0, KType::NUMBER).raw();
    assert_eq!(
        solve_over(
            &types,
            region,
            &[variable, variable],
            &[KType::NUMBER],
            &[KType::NUMBER.raw(), KType::STR.raw()]
        ),
        Err(Some(0))
    );
    let takes = |t: Handle| types.function_type(region, &[(x, t)], KType::NULL.raw());
    assert_eq!(
        solve_over(
            &types,
            region,
            &[takes(variable)],
            &[KType::NUMBER],
            &[takes(Handle::ANY)]
        ),
        Ok(vec![KType::NUMBER.raw()])
    );
}

/// The item's chain through a joined instance: a function generic over both its parameters fits
/// its instance at `Number | Str`, which lies under a function taking a number and a string — and
/// *fits* reaches the last from the first in one step. The order relates only the last two.
#[test]
fn fits_closes_through_a_joined_instance() {
    let symbols = SymbolInterner::new();
    let bump = Bump::new();
    let region = &bump;
    let types = TypeRegistry::in_region(region);
    let x = BinderSymbol::declared("x", &symbols).expect("a bindable token");
    let y = BinderSymbol::declared("y", &symbols).expect("a bindable token");
    let elt = TypeSymbol::declared("Elt", &symbols).expect("a Type token");
    let takes = |group: &[TypeSymbol], first: Handle, second: Handle| {
        types
            .function_group(
                region,
                group,
                &[KType::ANY][..group.len()],
                &[(x, first), (y, second)],
                KType::NULL.raw(),
            )
            .0
    };
    let variable = types.quantified(0, KType::ANY).raw();
    let generic = takes(&[elt], variable, variable);
    let number_or_str = types.union_of(region, &[KType::NUMBER.raw(), KType::STR.raw()]);
    let joined = takes(&[], number_or_str, number_or_str);
    let split = takes(&[], KType::NUMBER.raw(), KType::STR.raw());
    assert!(fits(&types, region, generic, joined));
    assert!(is_subtype_of(&types, region, joined, split));
    assert!(fits(&types, region, generic, split));
    assert!(!is_subtype_of(&types, region, generic, joined));
}

/// Admit each of `arguments` into its slot of `slots` through one collector bounded by `bounds`,
/// then solve. `Ok` carries the solution; `Err` carries the index of a variable whose pair denotes
/// nothing, or `None` for a mismatch.
fn solve_over(
    types: &TypeRegistry<'_>,
    region: &Bump,
    slots: &[Handle],
    bounds: &[KType],
    arguments: &[Handle],
) -> Result<Vec<Handle>, Option<usize>> {
    let mut collector = Collector::<Handle>::new(region, bounds);
    for (slot, argument) in slots.iter().zip(arguments) {
        if admits(
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

/// No law: a worked spelling. A variable one position names stays a variable, and one no position
/// names stays in the group behind the rest, its bound part of the type's identity.
#[test]
fn a_binder_keeps_every_variable_it_declares() {
    let symbols = SymbolInterner::new();
    let bump = Bump::new();
    let region = &bump;
    let types = TypeRegistry::in_region(region);
    let keyword = KeywordSymbol::declared("PURE", &symbols).expect("a keyword token");
    let elt = TypeSymbol::declared("Elt", &symbols).expect("a Type token");
    let other = TypeSymbol::declared("Other", &symbols).expect("a Type token");
    let head = |names: &[TypeSymbol], bounds: &[KType], slot: Handle, ret: Handle| {
        types
            .shape_group(
                region,
                names,
                bounds,
                &[
                    DispatchTokenElement::Keyword(keyword),
                    DispatchTokenElement::Slot(slot),
                ],
                &[],
                ret,
            )
            .0
    };
    let in_slot = head(
        &[elt],
        &[KType::NUMBER],
        types.quantified(0, KType::NUMBER).raw(),
        KType::NULL.raw(),
    );
    assert_ne!(
        in_slot,
        head(&[], &[], KType::NUMBER.raw(), KType::NULL.raw())
    );
    let renamed = head(
        &[other],
        &[KType::NUMBER],
        types.quantified(0, KType::NUMBER).raw(),
        KType::NULL.raw(),
    );
    assert_eq!(in_slot, renamed, "the names are render-only");
    let TypeNode::ExpressionShape { bounds, .. } = types.node(in_slot) else {
        panic!("the shape door interned something else");
    };
    assert_eq!(bounds, &[KType::NUMBER]);
    let in_return = head(
        &[elt],
        &[KType::NUMBER],
        KType::STR.raw(),
        types.quantified(0, KType::NUMBER).raw(),
    );
    assert_ne!(in_return, head(&[], &[], KType::STR.raw(), Handle::NEVER));

    // `Other` is named nowhere. Declared first or second, it is numbered after `Elt`.
    let first = head(
        &[elt, other],
        &[KType::NUMBER, KType::STR],
        types.quantified(0, KType::NUMBER).raw(),
        KType::NULL.raw(),
    );
    let second = head(
        &[other, elt],
        &[KType::STR, KType::NUMBER],
        types.quantified(1, KType::NUMBER).raw(),
        KType::NULL.raw(),
    );
    assert_eq!(first, second);
    assert_ne!(first, in_slot);
    let other_bound = head(
        &[elt, other],
        &[KType::NUMBER, KType::BOOL],
        types.quantified(0, KType::NUMBER).raw(),
        KType::NULL.raw(),
    );
    assert_ne!(
        first, other_bound,
        "an unnamed variable's bound is identity"
    );
}

/// No law: the elaborator never builds a group-free shape over an enclosing group's variable, since
/// an `EXPR` with no `FOR ALL` refuses outer names, so no generated type puts one there either. A
/// shape with no group binds nothing: a variable inside one is the enclosing binder's, numbered by
/// its occurrence there, and substitution reaches it.
#[test]
fn a_shape_with_no_group_is_transparent() {
    let symbols = SymbolInterner::new();
    let bump = Bump::new();
    let region = &bump;
    let types = TypeRegistry::in_region(region);
    let keyword = KeywordSymbol::declared("PURE", &symbols).expect("a keyword token");
    let elt = TypeSymbol::declared("Elt", &symbols).expect("a Type token");
    let other = TypeSymbol::declared("Other", &symbols).expect("a Type token");
    let shape = |names: &[TypeSymbol], slots: &[Handle], ret: Handle| {
        let mut run = Vec::new();
        for slot in slots {
            run.push(DispatchTokenElement::Keyword(keyword));
            run.push(DispatchTokenElement::Slot(*slot));
        }
        types.shape_group(
            region,
            names,
            &vec![KType::ANY; names.len()],
            &run,
            &[],
            ret,
        )
    };
    // `Other`, declared second, is named first: inside the group-free slot.
    let inner = shape(
        &[],
        &[types.quantified(1, KType::ANY).raw()],
        KType::NULL.raw(),
    )
    .0;
    assert!(!types.node(inner).binds_quantifiers());
    let scheme = shape(
        &[elt, other],
        &[inner, types.quantified(0, KType::ANY).raw()],
        KType::NULL.raw(),
    );
    assert_eq!(scheme.1, &[1, 0]);
    let first = shape_slots(scheme.0, &types).next().expect("a slot");
    let opened = crate::type_lattice::substitute::substitute_quantified(
        &types,
        region,
        first,
        &[KType::NUMBER.raw(), KType::STR.raw()],
    );
    assert_eq!(
        opened,
        shape(&[], &[KType::NUMBER.raw()], KType::NULL.raw()).0
    );
    assert_eq!(
        display_handle(scheme.0, &types, &symbols).to_string(),
        ":(EXPR FOR ALL #[Other Elt] #(PURE _ :(EXPR #(PURE _ :Other) -> Null) PURE _ :Elt) -> Null)"
    );
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
    let forwards = [(x, KType::NUMBER.raw()), (y, KType::STR.raw())];
    let backwards = [(y, KType::STR.raw()), (x, KType::NUMBER.raw())];
    assert_eq!(Record::<Handle>::over(&forwards), Record::over(&backwards));
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
    let wide = types.record(region, &[(x, KType::NUMBER.raw()), (y, KType::STR.raw())]);
    let narrow = types.record(region, &[(x, KType::NUMBER.raw())]);
    assert!(
        is_subtype_of(&types, region, wide, narrow),
        "records are width-superset"
    );
    assert!(!is_subtype_of(&types, region, narrow, wide));
    assert_eq!(meet(&types, region, wide, narrow), wide);
    assert_eq!(join(&types, region, wide, narrow), narrow);

    let few = types.function_type(region, &[(x, KType::NUMBER.raw())], KType::NULL.raw());
    let many = types.function_type(
        region,
        &[(x, KType::NUMBER.raw()), (y, KType::STR.raw())],
        KType::NULL.raw(),
    );
    assert!(
        is_subtype_of(&types, region, few, many),
        "a function subtype asks for no name the supertype does not",
    );
    assert!(!is_subtype_of(&types, region, many, few));
}

/// No law: a quantified function's identity is a claim about worked spellings — which renamings
/// and which written orders collapse to one handle — and a generated pair almost never spells two
/// of them.
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
    let variable = types.quantified(0, KType::ANY).raw();

    // `FN FOR ALL (Elt) :{x :Elt} -> Elt` under either spelling, and with the fields in either
    // written order, is one handle: the names are render-only and the record is order-blind.
    let identity = |group, param| {
        types
            .function_group(region, group, &[KType::ANY], &[(x, param)], param)
            .0
    };
    let (as_elt, as_other) = ([elt], [other]);
    assert_eq!(identity(&as_elt, variable), identity(&as_other, variable));
    let forward = types
        .function_group(
            region,
            &[elt],
            &[KType::ANY],
            &[(x, variable), (y, KType::STR.raw())],
            variable,
        )
        .0;
    let reversed = types
        .function_group(
            region,
            &[elt],
            &[KType::ANY],
            &[(y, KType::STR.raw()), (x, variable)],
            variable,
        )
        .0;
    assert_eq!(forward, reversed);

    // *Fits* instantiates: the quantified identity fits every monomorphic identity, and a quantified
    // function that promises less about its return. The order relates neither pair.
    let quantified_identity = identity(&as_elt, variable);
    let on_numbers = types.function_type(region, &[(x, KType::NUMBER.raw())], KType::NUMBER.raw());
    assert!(fits(&types, region, quantified_identity, on_numbers));
    assert!(!is_subtype_of(
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
        .function_group(region, &[elt], &[KType::ANY], &[(x, variable)], Handle::ANY)
        .0;
    assert!(fits(&types, region, quantified_identity, to_any));
    assert!(!is_subtype_of(&types, region, quantified_identity, to_any));
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
    let tops = [Handle::ANY_VALUE, Handle::ANY_TYPE, Handle::ANY_CODE];
    let under = |ktype: Handle| -> Vec<Handle> {
        tops.into_iter()
            .filter(|top| is_subtype_of(&types, region, ktype, *top))
            .collect()
    };

    let values = [
        KType::NUMBER.raw(),
        KType::STR.raw(),
        KType::BOOL.raw(),
        KType::NULL.raw(),
        KType::LIST_OF_ANY.raw(),
        KType::DICT_ANY_ANY.raw(),
        types.record(region, &[(x, Handle::ANY)]),
        types.function_type(region, &[(x, Handle::ANY)], Handle::ANY),
        types.intern(
            region,
            TypeNode::ExpressionShape {
                quantifiers: &[],
                bounds: &[],
                elements: Elements::over(&elements),
                classes: &[],
                ret: KType::NUMBER.raw(),
            },
        ),
        types.constructor_apply(region, KType::NUMBER, &[(x, Handle::ANY)]),
        Handle::EMPTY_SIGNATURE,
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
        types.sibling(0).raw(),
        Handle::ANY_VALUE,
    ];
    for value in values {
        assert_eq!(under(value), [Handle::ANY_VALUE], "{value:?}");
    }
    let codes = [
        Handle::IDENTIFIER,
        Handle::SYMBOL,
        Handle::TYPE_NAME_TOKEN,
        Handle::EXPRESSION,
        Handle::SIGILED_TYPE_EXPR,
        Handle::RECORD_TYPE,
        Handle::LITERAL,
        Handle::BLOCK,
        Handle::DECLARATION,
        Handle::BINDER,
        Handle::NAME,
        Handle::KEYWORD,
        Handle::ANY_CODE,
    ];
    for code in codes {
        assert_eq!(under(code), [Handle::ANY_CODE], "{code:?}");
    }
    for kind in [
        KKind::ProperType,
        KKind::Signature,
        KKind::AnyType,
        KKind::NewType,
        KKind::TypeConstructor,
    ] {
        assert_eq!(under(KType::of_kind(kind).raw()), [Handle::ANY_TYPE]);
    }

    // A variable answers by its bound; one bounded by `Any` — a sealed member's default — lies
    // under no family top, since the view hides which family it stands for.
    assert!(under(types.quantified(0, KType::ANY).raw()).is_empty());
    assert_eq!(
        under(types.quantified(0, KType::ANY_VALUE).raw()),
        [Handle::ANY_VALUE]
    );
    let sealed = types.parameter(name, KType::ANY, None);
    assert!(under(sealed).is_empty());
    // A union answers by its members, and a deferred return, whose return is unknown, by none.
    let mixed = types.union_of(region, &[KType::NUMBER.raw(), KType::PROPER_TYPE.raw()]);
    assert!(under(mixed).is_empty());
    let pair_top = types.union_of(region, &[Handle::ANY_VALUE, Handle::ANY_TYPE]);
    assert!(is_subtype_of(&types, region, mixed, pair_top));
    assert!(
        under(
            types
                .deferred_return(DeferredReturnSurface::Type(name))
                .raw()
        )
        .is_empty()
    );
    assert!(under(Handle::ANY).is_empty());

    // The families are disjoint and join to their union.
    assert_eq!(
        meet(&types, region, Handle::ANY_VALUE, Handle::ANY_CODE),
        Handle::NEVER
    );
    assert_eq!(
        join(&types, region, Handle::ANY_VALUE, Handle::ANY_TYPE),
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
    let below = |a: Handle, b: Handle| is_subtype_of(&types, region, a, b);
    let edges = [
        (Handle::BLOCK, Handle::ANY_CODE),
        (Handle::EXPRESSION, Handle::BLOCK),
        (Handle::DECLARATION, Handle::EXPRESSION),
        (Handle::LITERAL, Handle::EXPRESSION),
        (Handle::SYMBOL, Handle::EXPRESSION),
        (Handle::SIGILED_TYPE_EXPR, Handle::EXPRESSION),
        (Handle::RECORD_TYPE, Handle::EXPRESSION),
        (Handle::BINDER, Handle::DECLARATION),
        (Handle::NAME, Handle::SYMBOL),
        (Handle::KEYWORD, Handle::SYMBOL),
        (Handle::IDENTIFIER, Handle::NAME),
        (Handle::TYPE_NAME_TOKEN, Handle::NAME),
    ];
    for (child, parent) in edges {
        assert!(below(child, parent), "{child:?} under {parent:?}");
        assert!(!below(parent, child), "{parent:?} not under {child:?}");
    }
    let chain = [
        Handle::IDENTIFIER,
        Handle::NAME,
        Handle::SYMBOL,
        Handle::EXPRESSION,
        Handle::BLOCK,
        Handle::ANY_CODE,
    ];
    for (index, lower) in chain.iter().enumerate() {
        for upper in &chain[index..] {
            assert!(below(*lower, *upper), "{lower:?} under {upper:?}");
        }
    }
    assert!(below(Handle::BINDER, Handle::EXPRESSION));
    for (a, b) in [
        (Handle::KEYWORD, Handle::NAME),
        (Handle::LITERAL, Handle::SYMBOL),
        (Handle::BINDER, Handle::SYMBOL),
        (Handle::DECLARATION, Handle::BINDER),
        (Handle::BLOCK, Handle::EXPRESSION),
        (Handle::EXPRESSION, Handle::SYMBOL),
        (Handle::IDENTIFIER, Handle::KEYWORD),
    ] {
        assert!(!below(a, b), "{a:?} not under {b:?}");
    }
    for kind in [Handle::BLOCK, Handle::NAME, Handle::KEYWORD, Handle::BINDER] {
        assert!(!below(kind, Handle::ANY_VALUE), "{kind:?} not a value");
        assert!(!below(kind, Handle::ANY_TYPE), "{kind:?} not a type");
    }

    assert_eq!(
        meet(&types, region, Handle::EXPRESSION, Handle::BLOCK),
        Handle::EXPRESSION
    );
    assert_eq!(
        meet(&types, region, Handle::NAME, Handle::KEYWORD),
        Handle::NEVER
    );
    assert_eq!(
        meet(&types, region, Handle::LITERAL, Handle::BINDER),
        Handle::NEVER
    );
    assert_eq!(
        meet(&types, region, Handle::DECLARATION, Handle::SYMBOL),
        Handle::NEVER
    );
    assert_eq!(
        types.union_of(region, &[Handle::LITERAL, Handle::EXPRESSION]),
        Handle::EXPRESSION
    );

    let list_of_code = types.list(Handle::ANY_CODE);
    assert!(below(Handle::LIST_OF_NAME, list_of_code));
    assert!(below(list_of_code, Handle::ANY_VALUE));
    assert!(below(
        KType::LIST_OF_DECLARATION.raw(),
        types.list(Handle::EXPRESSION)
    ));
    assert!(below(
        KType::DICT_NAME_BLOCK.raw(),
        types.dict(Handle::SYMBOL, Handle::ANY_CODE)
    ));
    assert!(below(KType::TYPE_CODE.raw(), Handle::EXPRESSION));
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
    let below = |a: KType, b: KType| typed::is_subtype_of(&types, region, a, b);
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

    assert_eq!(
        typed::meet(&types, region, expression_yz, block_y),
        expression_y
    );
    assert_eq!(
        typed::meet(&types, region, expression_y, expression_z),
        KType::EXPRESSION
    );
    assert_eq!(
        typed::meet(&types, region, block_y, KType::LITERAL),
        KType::LITERAL
    );
    assert_eq!(
        typed::meet(&types, region, binder_y, KType::LITERAL),
        KType::NEVER
    );
    assert_eq!(
        typed::join(&types, region, expression_y, expression_yz),
        expression_yz
    );

    assert_eq!(
        display_handle(block_y.raw(), &types, &symbols).to_string(),
        ":(Block NEEDING #[y])"
    );
}

/// No law: the order's laws hold over concrete types, and these pin the worked examples — a variable
/// bounded by `Number | Str` against the unions above and around its bound, in the order, a union's
/// canonical form and the meet.
#[test]
fn a_union_bounded_variable_lies_under_every_union_above_its_bound() {
    let bump = Bump::new();
    let region = &bump;
    let types = TypeRegistry::in_region(region);
    let number_or_str = types.union_of(region, &[KType::NUMBER, KType::STR]);
    let elt = types.quantified(0, number_or_str).raw();
    let number_or_str = number_or_str.raw();
    let wider = types.union_of(
        region,
        &[KType::NUMBER.raw(), KType::STR.raw(), KType::BOOL.raw()],
    );
    assert!(is_subtype_of(&types, region, elt, number_or_str));
    assert!(is_subtype_of(&types, region, elt, wider));
    assert!(!is_subtype_of(&types, region, elt, KType::NUMBER.raw()));

    // `Elt | Number | Str` holds nothing `Number | Str` does not, but the order relates concrete
    // types only, so the variable stays beside the members its bound lies under.
    let spelled = types.union_of(region, &[elt, KType::NUMBER.raw(), KType::STR.raw()]);
    assert!(
        matches!(types.node(spelled), TypeNode::Union { members } if members[..] == [elt, KType::NUMBER.raw(), KType::STR.raw()])
    );
    let elt_or_bool = types.union_of(region, &[elt, KType::BOOL.raw()]);
    assert_ne!(elt_or_bool, types.union_of(region, &[KType::BOOL.raw()]));

    // The meet keeps a variable whose bound spans the other side's members, from either side.
    assert_eq!(meet(&types, region, elt_or_bool, number_or_str), elt);
    assert_eq!(meet(&types, region, number_or_str, elt_or_bool), elt);
}

/// No law: the generators rarely draw a carrier beside every member its bound spans. An opaque
/// carrier is concrete, so the order reduces it like any concrete member, under the union of the
/// rest as well as under one member.
#[test]
fn a_carrier_under_the_rest_of_a_union_is_dropped() {
    let symbols = SymbolInterner::new();
    let bump = Bump::new();
    let region = &bump;
    let types = TypeRegistry::in_region(region);
    let name = TypeSymbol::declared("Carrier", &symbols).expect("a Type token");
    let number_or_str = types.union_of(region, &[KType::NUMBER, KType::STR]);
    let carrier = types.carrier(name, number_or_str, crate::memory::ScopeId::from_raw(1, 1));
    assert_eq!(
        types.union_of(region, &[carrier, KType::NUMBER, KType::STR]),
        number_or_str
    );
    assert_eq!(
        typed::join(&types, region, carrier, number_or_str),
        number_or_str
    );
}

/// No law: the unifier's carried-variable rule is covered by a property, but the solution it
/// reaches is a worked example. `Y` bounded by `LIST OF Number` fills `LIST OF X`, solving `X` to
/// `Number` — and that closes *fits*' transitivity through a monomorphic instance.
#[test]
fn a_carried_variable_fills_what_its_bound_fills() {
    let symbols = SymbolInterner::new();
    let bump = Bump::new();
    let region = &bump;
    let types = TypeRegistry::in_region(region);
    let name = TypeSymbol::declared("Held", &symbols).expect("a Type token");
    let list_of_number = types.list(KType::NUMBER);
    let carried = types.parameter(name, list_of_number, None);
    let declared = types.list(types.quantified(0, KType::ANY).raw());
    let mut collector = Collector::<Handle>::new(region, &[KType::ANY]);
    assert_eq!(
        admits(
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
        Ok(vec![KType::NUMBER.raw()])
    );

    // `FN FOR ALL #[Elt] :{y :(LIST OF Elt) z :(LIST OF Elt)} -> Null` fits the `Number` instance,
    // which fits `FN FOR ALL #{Held: :(LIST OF Number)} :{y :Held z :Held} -> Null`, and the first
    // fits the third.
    let y = BinderSymbol::declared("y", &symbols).expect("a bindable token");
    let z = BinderSymbol::declared("z", &symbols).expect("a bindable token");
    let x_name = TypeSymbol::declared("Elt", &symbols).expect("a Type token");
    let each_list = types
        .function_group(
            region,
            &[x_name],
            &[KType::ANY],
            &[(y, declared), (z, declared)],
            KType::NULL.raw(),
        )
        .0;
    let on_numbers = types.function_type(
        region,
        &[(y, list_of_number.raw()), (z, list_of_number.raw())],
        KType::NULL.raw(),
    );
    let bounded = types.quantified(0, list_of_number).raw();
    let each_bounded = types
        .function_group(
            region,
            &[name],
            &[list_of_number],
            &[(y, bounded), (z, bounded)],
            KType::NULL.raw(),
        )
        .0;
    // A bound spanning two declared members is admitted member by member: `Held` bounded by
    // `LIST OF Number | Str` fills `LIST OF X | Str`, though neither member alone takes it.
    let spanning_bound = types.union_of(region, &[list_of_number, KType::STR]);
    let spanning = types.parameter(name, spanning_bound, None);
    let either = types.union_of(region, &[declared, KType::STR.raw()]);
    let mut collector = Collector::<Handle>::new(region, &[KType::ANY]);
    assert_eq!(
        admits(
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
        Ok(vec![KType::NUMBER.raw()])
    );

    assert!(fits(&types, region, each_list, on_numbers));
    assert!(fits(&types, region, on_numbers, each_bounded));
    assert!(fits(&types, region, each_list, each_bounded));
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
    let deferred = types
        .deferred_return(DeferredReturnSurface::Type(name))
        .raw();
    let admits = |declared: Handle| {
        let mut collector = Collector::<Handle>::new(region, &[KType::ANY]);
        admits(
            &types,
            region,
            declared,
            deferred,
            Variance::Co,
            &mut collector,
        )
    };
    let variable = types.quantified(0, KType::ANY).raw();
    assert_eq!(admits(types.list(variable)), Err(UnifyFailure::Mismatch));
    assert_eq!(admits(types.list(Handle::ANY)), Err(UnifyFailure::Mismatch));
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
    let value_bounded = types.quantified(0, KType::ANY_VALUE).raw();
    let free = types.quantified(1, KType::ANY).raw();
    let pair = types
        .function_group(
            region,
            &[elt, key],
            &[KType::ANY_VALUE, KType::ANY],
            &[(a, value_bounded), (b, free)],
            types.dict(value_bounded, free),
        )
        .0;
    // The group is ordered by first occurrence, which puts `Key` first here.
    assert_eq!(
        display_handle(pair, &types, &symbols).to_string(),
        ":(FN FOR ALL #{Key: Any, Elt: Value} :{a :Elt b :Key} -> :(MAP Elt -> Key))"
    );
    let alone = types
        .function_group(
            region,
            &[elt],
            &[KType::ANY_VALUE],
            &[(a, value_bounded)],
            value_bounded,
        )
        .0;
    let rendered = display_handle(alone, &types, &symbols).to_string();
    assert!(rendered.contains("FOR ALL #{Elt: Value} "), "{rendered}");
    let number_or_str = types.union_of(region, &[KType::NUMBER, KType::STR]);
    let union_bounded = types.quantified(0, number_or_str).raw();
    let spanning = types
        .function_group(
            region,
            &[elt],
            &[number_or_str],
            &[(a, union_bounded)],
            union_bounded,
        )
        .0;
    let rendered = display_handle(spanning, &types, &symbols).to_string();
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
    let above = |kt| crate::type_lattice::substitute::bound_above(&types, region, kt);
    let y = BinderSymbol::declared("y", &symbols).expect("a bindable token");
    let tee = TypeSymbol::declared("Tee", &symbols).expect("a Type token");
    let elt = types.quantified(0, KType::ANY).raw();

    assert_eq!(above(types.list(elt)), types.list(Handle::ANY));
    let taking = types.function_type(region, &[(y, elt)], elt);
    let widest = types.function_type(region, &[(y, Handle::NEVER)], Handle::ANY);
    assert_eq!(above(taking), widest);
    let number_or_str = types.union_of(region, &[KType::NUMBER, KType::STR]);
    assert_eq!(
        above(types.quantified(0, number_or_str).raw()),
        number_or_str.raw()
    );

    let closed = types
        .function_group(region, &[tee], &[KType::ANY], &[(y, elt)], elt)
        .0;
    assert_eq!(above(closed), closed);
    let list_of_number = types.list(KType::NUMBER.raw());
    assert_eq!(above(list_of_number), list_of_number);
}

/// `#(<keyword> _ :<slot>) -> <ret>`, unranked and unquantified.
fn head(
    types: &TypeRegistry<'_>,
    region: &Bump,
    keyword: KeywordSymbol,
    slot: Handle,
    ret: Handle,
) -> Handle {
    types.shape_type(
        region,
        &[
            DispatchTokenElement::Keyword(keyword),
            DispatchTokenElement::Slot(slot),
        ],
        &[],
        ret,
    )
}

/// A signature over `shapes`, declared over `parameters` or a module's when `module`.
fn keyworded(
    types: &TypeRegistry<'_>,
    region: &Bump,
    module: bool,
    parameters: &[(TypeSymbol, Parametric)],
    shapes: &[Handle],
) -> KType {
    let mut draft = SchemaDraft::new(region);
    if !module {
        draft.origin = SigOrigin::Declared;
    }
    for (name, parameter) in parameters {
        draft.insert_parameter(*name, *parameter);
    }
    for shape in shapes {
        draft.push_keyworded(types.declared(*shape));
    }
    types.signature(region, draft)
}

/// No law: *fits* is transitive, and these are the worked spellings that would break it were a
/// key's overloads pooled into one solve. A module with `PUSH` at `Number` and at `Str` fits each
/// pinned application of `Stack` and their meet, which lies under `Stack`, so it fits `Stack` too —
/// at the first overload's `Elt`. One with only the `Number` overload fits `Stack` and its
/// `Number` application, and not its `Str` one.
#[test]
fn a_key_s_overloads_are_tried_one_at_a_time() {
    let symbols = SymbolInterner::new();
    let bump = Bump::new();
    let region = &bump;
    let types = TypeRegistry::in_region(region);
    let push = KeywordSymbol::declared("PUSH", &symbols).expect("a keyword token");
    let elt = TypeSymbol::declared("Elt", &symbols).expect("a Type token");
    let parameter = types.head_parameter(elt, KType::ANY);
    let raw = parameter.raw();
    let stack = keyworded(
        &types,
        region,
        false,
        &[(elt, parameter)],
        &[head(&types, region, push, raw, types.list(raw))],
    );
    let pin = |kt| types.signature_apply(region, stack, &[(BinderSymbol::Type(elt), kt)]);
    let (numbers, strs) = (pin(KType::NUMBER), pin(KType::STR));
    let both = typed::meet(&types, region, numbers, strs);
    let at = |kt| head(&types, region, push, kt, types.list(kt));
    let two = keyworded(
        &types,
        region,
        true,
        &[],
        &[at(KType::NUMBER.raw()), at(KType::STR.raw())],
    );
    let one = keyworded(&types, region, true, &[], &[at(KType::NUMBER.raw())]);
    for asked in [stack, numbers, strs, both] {
        assert!(typed::sig_fits(&types, region, two, asked).is_ok());
    }
    assert!(typed::is_subtype_of(&types, region, both, stack));
    assert!(typed::sig_fits(&types, region, one, stack).is_ok());
    assert!(typed::sig_fits(&types, region, one, numbers).is_ok());
    assert!(typed::sig_fits(&types, region, one, strs).is_err());
}

/// No law: *fits* asks only that some overload satisfy each keyworded member, so a tie is no
/// refusal. `M`'s two `FIT`s both satisfy `Narrow`'s, and `M` fits `Wide`, which fits `Narrow`.
#[test]
fn a_tie_under_an_asked_member_still_fits() {
    let symbols = SymbolInterner::new();
    let bump = Bump::new();
    let region = &bump;
    let types = TypeRegistry::in_region(region);
    let f = KeywordSymbol::declared("FIT", &symbols).expect("a keyword token");
    let either = |other| types.union_of(region, &[KType::NUMBER.raw(), other]);
    let at = |slot| head(&types, region, f, slot, KType::NUMBER.raw());
    let m = keyworded(
        &types,
        region,
        true,
        &[],
        &[at(either(KType::STR.raw())), at(either(KType::BOOL.raw()))],
    );
    let wide = keyworded(&types, region, false, &[], &[at(either(KType::STR.raw()))]);
    let narrow = keyworded(&types, region, false, &[], &[at(KType::NUMBER.raw())]);
    assert!(typed::sig_fits(&types, region, m, wide).is_ok());
    assert!(typed::sig_fits(&types, region, wide, narrow).is_ok());
    assert!(typed::sig_fits(&types, region, m, narrow).is_ok());
}

/// No law: the item's worked spellings of the two relations. The order compares applications by
/// their pins and two quantified function types by handle; *fits* solves where the order will not,
/// and records its verdict under its own relation.
#[test]
fn the_order_never_solves_and_fits_does() {
    let symbols = SymbolInterner::new();
    let bump = Bump::new();
    let region = &bump;
    let types = TypeRegistry::in_region(region);
    let below = |a: Handle, b: Handle| is_subtype_of(&types, region, a, b);

    // `Stack WITH {Elt = Number}` lies under `Stack`; its `Str` application is unordered with it,
    // and the meet of the two lies under each.
    let push = KeywordSymbol::declared("PUSH", &symbols).expect("a keyword token");
    let elt = TypeSymbol::declared("Elt", &symbols).expect("a Type token");
    let parameter = types.head_parameter(elt, KType::ANY);
    let raw = parameter.raw();
    let stack = keyworded(
        &types,
        region,
        false,
        &[(elt, parameter)],
        &[head(&types, region, push, raw, types.list(raw))],
    );
    let pin = |kt| types.signature_apply(region, stack, &[(BinderSymbol::Type(elt), kt)]);
    let (numbers, strs) = (pin(KType::NUMBER), pin(KType::STR));
    let (stack, numbers, strs) = (stack.raw(), numbers.raw(), strs.raw());
    assert!(below(numbers, stack));
    assert!(!below(stack, numbers));
    assert!(!below(numbers, strs) && !below(strs, numbers));
    let both = meet(&types, region, numbers, strs);
    assert!(below(both, numbers) && below(both, strs));

    // `∀Elt :{x :Elt, y :Elt} -> Elt` and `∀A B :{x :A, y :B} -> A | B` are unordered, and the
    // first fits the second at its instance over `A | B`.
    let x = BinderSymbol::declared("x", &symbols).expect("a bindable token");
    let y = BinderSymbol::declared("y", &symbols).expect("a bindable token");
    let left = TypeSymbol::declared("Left", &symbols).expect("a Type token");
    let right = TypeSymbol::declared("Right", &symbols).expect("a Type token");
    let (first, second) = (
        types.quantified(0, KType::ANY).raw(),
        types.quantified(1, KType::ANY).raw(),
    );
    let same = types
        .function_group(
            region,
            &[elt],
            &[KType::ANY],
            &[(x, first), (y, first)],
            first,
        )
        .0;
    let either = types
        .function_group(
            region,
            &[left, right],
            &[KType::ANY, KType::ANY],
            &[(x, first), (y, second)],
            types.union_of(region, &[first, second]),
        )
        .0;
    assert!(!below(same, either) && !below(either, same));
    assert!(fits(&types, region, same, either));

    // One `fits` leaves its verdict under its own relation, apart from the order's.
    assert_eq!(
        types.verdict(same.digest(), either.digest(), Relation::Fits),
        Some(true)
    );
    assert_eq!(
        types.verdict(same.digest(), either.digest(), Relation::Subtype),
        Some(false)
    );
}
