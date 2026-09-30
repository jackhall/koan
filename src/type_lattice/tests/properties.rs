//! The laws.
//!
//! Every statement here is a property of the algebra, not a pin on a particular input: a rewrite of
//! any walk has these as its oracle. One registry per test thread, so handles generated inside one
//! law all name content the same table holds; each case brings its own scratch region, dropped with
//! the case.

use proptest::prelude::*;
use proptest::strategy::ValueTree;

use crate::memory::{Bump, BumpAllocator, BumpVec, ScopeId};
use crate::symbols::TypeSymbol;
use crate::type_lattice::handle::KType;
use crate::type_lattice::kind::KKind;
use crate::type_lattice::lattice::{join, join_iter, meet};
use crate::type_lattice::node::TypeNode;
use crate::type_lattice::order::{is_more_specific_than, is_subtype_of, satisfied_by};
use crate::type_lattice::ranking::{Verdict, admit_by_class, judge_by_class};
use crate::type_lattice::registry::TypeRegistry;
use crate::type_lattice::schema::{
    Members, SigSchema, canonical_overloads, is_shape, shape_classes, shape_keys_equal,
    shape_quantifiers, shape_return, shape_slots, specialize_schema,
};
use crate::type_lattice::shape::Specificity;
use crate::type_lattice::sig_relations::{
    admits_shape, meet_schemas, shape_specificity, sig_subtype,
};
use crate::type_lattice::substitute::{
    Side, bound_above, canonicalize_binder, erase_quantified, instantiate_quantified,
    quantifier_bounds, read_through, slot_more_specific_or_equal, slot_satisfied_by,
    slot_types_equal, substitute_quantified, substitute_sig_members,
};
use crate::type_lattice::unify::{Collector, Interval, UnifyFailure, admits_with, intervals};
use crate::type_lattice::walk::Variance;
use crate::type_lattice::walk::unary::{LEAF, Visit, visit};
use crate::type_lattice::window::{RecursiveGroupWindow, RelativeSchema};

use super::generators::{World, arb_arguments, arb_function_type, arb_shape_type, arb_type};

thread_local! {
    /// One live registry and one alphabet per test thread — proptest runs each `#[test]` on its
    /// own, so a law's generated handles all name content one table holds.
    static WORLD: World = World::new();
}

fn world() -> World {
    WORLD.with(|world| world.clone())
}

fn registry() -> std::rc::Rc<TypeRegistry<'static>> {
    world().types
}

/// A generated type at the standard depth.
fn one() -> BoxedStrategy<KType> {
    arb_type(world(), 3)
}

/// A shallower one, for the ternary laws that would otherwise multiply three deep trees.
fn small() -> BoxedStrategy<KType> {
    arb_type(world(), 2)
}

/// A generated expression shape. The laws whose subject is a shape draw from here rather than from
/// the whole vocabulary, where most draws would satisfy them vacuously.
fn shape() -> BoxedStrategy<KType> {
    arb_shape_type(world(), 3)
}

/// A generated function type, for the same reason [`shape`] exists: the laws about a binder's
/// canonical form have nothing to say about a draw that is not one.
fn function() -> BoxedStrategy<KType> {
    arb_function_type(world(), 3)
}

/// The binary laws take the whole of whatever depth the tier asks for: the space two generated
/// types range over is the widest any law here draws from, and a thin sweep of it proves little.
fn binary() -> ProptestConfig {
    ProptestConfig {
        cases: crate::tests::case_share(1, 1),
        ..ProptestConfig::default()
    }
}

/// Three deep trees per case, so the ternary laws take three-eighths of the binary depth to land
/// near the same wall-clock — the ratio holds at whatever the tier sets.
fn ternary() -> ProptestConfig {
    ProptestConfig {
        cases: crate::tests::case_share(3, 8),
        ..ProptestConfig::default()
    }
}

// --- 1. Join and meet ---

proptest! {
    #![proptest_config(binary())]

    #[test]
    fn join_and_meet_are_commutative_and_idempotent(a in one(), b in one()) {
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        prop_assert_eq!(join(&types, scratch, a, b), join(&types, scratch, b, a));
        prop_assert_eq!(meet(&types, scratch, a, b), meet(&types, scratch, b, a));
        prop_assert_eq!(join(&types, scratch, a, a), a);
        prop_assert_eq!(meet(&types, scratch, a, a), a);
    }

    #[test]
    fn join_and_meet_absorb_each_other(a in one(), b in one()) {
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        prop_assert_eq!(join(&types, scratch, a, meet(&types, scratch, a, b)), a);
        prop_assert_eq!(meet(&types, scratch, a, join(&types, scratch, a, b)), a);
    }

    #[test]
    fn never_and_any_are_the_identities(a in one()) {
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        prop_assert_eq!(join(&types, scratch, a, KType::NEVER), a);
        prop_assert_eq!(meet(&types, scratch, a, KType::ANY), a);
    }
}

proptest! {
    #![proptest_config(ternary())]

    #[test]
    fn join_and_meet_are_associative(a in small(), b in small(), c in small()) {
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        prop_assert_eq!(
            join(&types, scratch, join(&types, scratch, a, b), c),
            join(&types, scratch, a, join(&types, scratch, b, c))
        );
        prop_assert_eq!(
            meet(&types, scratch, meet(&types, scratch, a, b), c),
            meet(&types, scratch, a, meet(&types, scratch, b, c))
        );
    }
}

// --- 2. The order ---

proptest! {
    #![proptest_config(binary())]

    #[test]
    fn the_order_is_reflexive_and_bounded(a in one()) {
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        prop_assert!(is_subtype_of(&types, scratch, a, a));
        prop_assert!(is_subtype_of(&types, scratch, KType::NEVER, a));
        prop_assert!(is_subtype_of(&types, scratch, a, KType::ANY));
        prop_assert!(!is_more_specific_than(&types, scratch, a, a));
    }

    #[test]
    fn the_order_is_antisymmetric(a in one(), b in one()) {
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        if is_subtype_of(&types, scratch, a, b) && is_subtype_of(&types, scratch, b, a) {
            prop_assert_eq!(a, b);
        }
    }

    #[test]
    fn the_order_agrees_with_join_and_meet(a in one(), b in one()) {
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        let below = is_subtype_of(&types, scratch, a, b);
        prop_assert_eq!(below, join(&types, scratch, a, b) == b);
        prop_assert_eq!(below, meet(&types, scratch, a, b) == a);
        prop_assert_eq!(below, satisfied_by(&types, scratch, b, a));
        prop_assert_eq!(is_more_specific_than(&types, scratch, a, b), a != b && below);
    }

    #[test]
    fn below_a_rigid_variable_lie_itself_and_what_lies_under_its_lower_end(
        a in one(),
        b in one(),
    ) {
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        if let Some(lower) = types.node(b).rigid_lower() {
            prop_assert_eq!(
                is_subtype_of(&types, scratch, a, b),
                a == b || is_subtype_of(&types, scratch, a, lower)
            );
        }
    }

    #[test]
    fn no_type_lies_below_two_family_tops(a in one()) {
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        let tops = [KType::ANY_VALUE, KType::ANY_TYPE, KType::ANY_CODE]
            .into_iter()
            .filter(|top| is_subtype_of(&types, scratch, a, *top))
            .count();
        prop_assert!(tops <= 1 || a == KType::NEVER, "a type lies below {} family tops", tops);
    }
}

proptest! {
    #![proptest_config(ternary())]

    #[test]
    fn the_order_is_transitive(a in small(), b in small(), c in small()) {
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        if is_subtype_of(&types, scratch, a, b) && is_subtype_of(&types, scratch, b, c) {
            prop_assert!(is_subtype_of(&types, scratch, a, c));
        }
    }
}

// --- 3. Unions ---

proptest! {
    #![proptest_config(binary())]

    #[test]
    fn union_of_is_canonical(members in prop::collection::vec(one(), 1..5)) {
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        let union = types.union_of(scratch, &members);
        let mut reversed = members.clone();
        reversed.reverse();
        prop_assert_eq!(union, types.union_of(scratch, &reversed));
        prop_assert_eq!(union, types.union_of(scratch, &[union]));
        // Flattening: wrapping the result in another union changes nothing.
        let mut nested = members.clone();
        nested.push(union);
        prop_assert_eq!(union, types.union_of(scratch, &nested));
        // Never is dropped and `Any` absorbs.
        let mut with_never = members.clone();
        with_never.push(KType::NEVER);
        prop_assert_eq!(union, types.union_of(scratch, &with_never));
        let mut with_any = members.clone();
        with_any.push(KType::ANY);
        prop_assert_eq!(KType::ANY, types.union_of(scratch, &with_any));
        // No member of a canonical union lies below the union of the others.
        if let TypeNode::Union { members: kept } = types.node(union) {
            for (index, member) in kept.iter().enumerate() {
                let others: Vec<KType> = kept
                    .iter()
                    .enumerate()
                    .filter(|(peer, _)| *peer != index)
                    .map(|(_, other)| *other)
                    .collect();
                let rest = types.union_of(scratch, &others);
                prop_assert!(
                    !is_subtype_of(&types, scratch, *member, rest),
                    "a canonical union kept a member below the rest",
                );
            }
        }
        // Every member is below the union, and the union is below anything all members are below.
        for member in &members {
            prop_assert!(is_subtype_of(&types, scratch, *member, union));
        }
    }
}

proptest! {
    #![proptest_config(binary())]

    #[test]
    fn a_union_holding_the_three_family_tops_is_any(
        members in prop::collection::vec(one(), 0..4),
    ) {
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        let tops = [KType::ANY_VALUE, KType::ANY_TYPE, KType::ANY_CODE];
        let mut tops_last = members.clone();
        tops_last.extend(tops);
        prop_assert_eq!(types.union_of(scratch, &tops_last), KType::ANY);
        let mut tops_first = tops.to_vec();
        tops_first.extend(members.iter().rev());
        prop_assert_eq!(types.union_of(scratch, &tops_first), KType::ANY);
    }
}

// --- 4. Interning ---

proptest! {
    #![proptest_config(binary())]

    #[test]
    fn equal_content_interns_once(a in one(), b in one()) {
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        prop_assert_eq!(types.list(a), types.list(a));
        prop_assert_eq!(types.dict(a, b), types.dict(a, b));
        // A node read back out and re-interned names the same handle.
        prop_assert_eq!(types.intern(scratch, types.node(a)), a);
        prop_assert_eq!(types.intern(scratch, types.node(b)), b);
    }

    /// The two probe flags interning stores beside a node answer what a walk over the type would:
    /// a free quantifier reachable without crossing a shape's binder, and any rigid variable
    /// reachable at all.
    #[test]
    fn the_probe_flags_are_their_walks(a in one()) {
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        let quantified = visit(&types, scratch, a, LEAF, &mut |_, node, _| match node {
            // Either binder's group is its own, so nothing under one is free here.
            _ if node.binds_quantifiers() => Visit::Skip,
            TypeNode::Quantified { .. } => Visit::Stop,
            _ => Visit::Descend,
        });
        let rigid = visit(&types, scratch, a, LEAF, &mut |_, node, _| match node {
            TypeNode::Quantified { .. }
            | TypeNode::Lexical { .. }
            | TypeNode::AbstractType { .. } => Visit::Stop,
            _ => Visit::Descend,
        });
        prop_assert_eq!(types.contains_quantified(a), quantified);
        prop_assert_eq!(types.contains_rigid(a), rigid);
    }
}

// --- 5. Substitution ---

proptest! {
    #![proptest_config(binary())]

    #[test]
    fn substitution_of_nothing_is_the_identity(a in one(), b in one()) {
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        prop_assert_eq!(substitute_quantified(&types, scratch, a, &[]), a);
        if !types.contains_quantified(a) {
            prop_assert_eq!(substitute_quantified(&types, scratch, a, &[b, b, b]), a);
        }
        prop_assert_eq!(
            substitute_sig_members(&types, scratch, a, ScopeId::SENTINEL, Members::EMPTY),
            a
        );
    }

    #[test]
    fn erasing_is_instantiating_at_the_bounds(a in one()) {
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        let bounds = quantifier_bounds(&types, a);
        prop_assert_eq!(
            erase_quantified(&types, scratch, a),
            instantiate_quantified(&types, scratch, a, bounds)
        );
    }

    // A type holding a binder is left out: relating two of them runs the unifier, whose
    // completeness is not this law's subject.
    #[test]
    fn bounding_above_lies_over_every_instance(a in one()) {
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        prop_assume!(!visit(&types, scratch, a, LEAF, &mut |_, node, _| {
            if node.binds_quantifiers() { Visit::Stop } else { Visit::Descend }
        }));
        let above = bound_above(&types, scratch, a);
        prop_assert!(!types.contains_rigid(above));
        prop_assert!(is_subtype_of(&types, scratch, a, above));
        let lowest = [KType::NEVER; 8];
        let instance = substitute_quantified(&types, scratch, a, &lowest);
        prop_assert!(is_subtype_of(&types, scratch, instance, above));
    }

    #[test]
    fn canonicalizing_a_binder_is_idempotent(a in one()) {
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        let once = canonicalize_binder(&types, scratch, a, ScopeId::SENTINEL);
        prop_assert_eq!(
            canonicalize_binder(&types, scratch, once, ScopeId::SENTINEL),
            once
        );
    }

    #[test]
    fn the_slot_relations_are_their_definitions(a in one(), b in one(), c in one()) {
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        let world = world();
        let members = Members::from_pairs(scratch, [(world.type_names[0], c)]);
        let id = ScopeId::SENTINEL;
        let substituted = substitute_sig_members(&types, scratch, a, id, members);
        prop_assert_eq!(
            slot_satisfied_by(&types, scratch, a, b, id, members),
            satisfied_by(&types, scratch, substituted, b)
        );
        prop_assert_eq!(
            slot_more_specific_or_equal(&types, scratch, a, b, id, members),
            is_subtype_of(&types, scratch, substituted, b)
        );
        prop_assert_eq!(
            slot_types_equal(&types, scratch, a, b, id, members),
            substituted == b
        );
    }
}

// --- 6. Shapes ---

proptest! {
    #![proptest_config(binary())]

    #[test]
    fn canonical_shape_form_is_a_fixed_point(a in shape()) {
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        if let TypeNode::ExpressionShape {
            quantifiers,
            elements,
            classes,
            ret,
            ..
        } = types.node(a)
        {
            let again = types.shape_type(scratch, quantifiers, elements, classes, ret);
            prop_assert_eq!(again.handle, a);
        }
    }

    /// The bounds a shape node stores beside its quantifier group are the ones its own occurrences
    /// carry — the canonical form guarantees every surviving variable occurs, so each is reachable.
    #[test]
    fn a_shape_stores_the_bounds_its_occurrences_carry(a in shape()) {
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        let stored = quantifier_bounds(&types, a);
        let mut carried: Vec<Option<KType>> = vec![None; stored.len()];
        visit(&types, scratch, a, LEAF, &mut |_, node, context| match *node {
            TypeNode::Quantified { index, bound } if context.binder_depth() == 1 => {
                if let Some(slot) = carried.get_mut(index) {
                    *slot = Some(bound);
                }
                Visit::Skip
            }
            // A nested group's variables are its own.
            _ if node.binds_quantifiers() && context.binder_depth() > 0 => Visit::Skip,
            _ => Visit::Descend,
        });
        for (index, bound) in stored.iter().enumerate() {
            prop_assert_eq!(carried[index], Some(*bound));
        }
    }

    /// Re-interning a function type through the canonicalizing door with the group it already
    /// carries returns the very handle: canonical form is idempotent, which is what makes the
    /// order over quantified functions antisymmetric.
    #[test]
    fn canonical_function_form_is_a_fixed_point(a in function()) {
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        if let TypeNode::KFunction {
            quantifiers,
            params,
            ret,
            ..
        } = types.node(a)
        {
            let again = types.function_type(scratch, quantifiers, params.as_slice(), ret);
            prop_assert_eq!(again.handle, a);
        }
    }

    /// The function twin of [`a_shape_stores_the_bounds_its_occurrences_carry`].
    #[test]
    fn a_function_stores_the_bounds_its_occurrences_carry(a in function()) {
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        let stored = quantifier_bounds(&types, a);
        let mut carried: Vec<Option<KType>> = vec![None; stored.len()];
        visit(&types, scratch, a, LEAF, &mut |_, node, context| match *node {
            TypeNode::Quantified { index, bound } if context.binder_depth() == 1 => {
                if let Some(slot) = carried.get_mut(index) {
                    *slot = Some(bound);
                }
                Visit::Skip
            }
            // A nested group's variables are its own.
            _ if node.binds_quantifiers() && context.binder_depth() > 0 => Visit::Skip,
            _ => Visit::Descend,
        });
        for (index, bound) in stored.iter().enumerate() {
            prop_assert_eq!(carried[index], Some(*bound));
        }
    }

    #[test]
    fn specificity_flips_when_its_arguments_swap(a in shape(), b in shape()) {
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        prop_assert_eq!(
            shape_specificity(&types, scratch, a, a),
            Specificity::Equal,
            "a shape is not `Equal` to itself"
        );
        let forward = shape_specificity(&types, scratch, a, b);
        let backward = shape_specificity(&types, scratch, b, a);
        let flipped = match forward {
            Specificity::StrictlyMore => Specificity::StrictlyLess,
            Specificity::StrictlyLess => Specificity::StrictlyMore,
            other => other,
        };
        prop_assert_eq!(
            flipped, backward,
            "specificity did not flip: forward {:?} backward {:?}",
            forward, backward
        );
    }

    #[test]
    fn specificity_refuses_anything_that_is_not_a_shape(a in one(), b in shape()) {
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        // Two things that are not both shapes share no bucket to rank under. The empty element run
        // a non-shape reads as would otherwise make every pair of leaves compare `Equal`.
        if !is_shape(a, &types) {
            prop_assert_eq!(shape_specificity(&types, scratch, a, b), Specificity::Incomparable);
            prop_assert_eq!(shape_specificity(&types, scratch, b, a), Specificity::Incomparable);
            prop_assert!(!admits_shape(&types, scratch, a, b));
            prop_assert!(!admits_shape(&types, scratch, b, a));
        }
    }

    /// Over monomorphic shapes the ranking is the pointwise order folded class by class: the first
    /// class whose slots order the pair one way only decides, and two rankings of one key are
    /// unrelated.
    #[test]
    fn monomorphic_specificity_is_the_lexicographic_pointwise_fold(a in shape(), b in shape()) {
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        let monomorphic = shape_quantifiers(a, &types).is_empty()
            && shape_quantifiers(b, &types).is_empty()
            && shape_return(a, &types).is_some()
            && shape_return(b, &types).is_some()
            && shape_keys_equal(a, b, &types);
        if !monomorphic {
            return Ok(());
        }
        let actual = shape_specificity(&types, scratch, a, b);
        let (ranking, other) = (shape_classes(a, &types), shape_classes(b, &types));
        if ranking != other {
            prop_assert_eq!(actual, Specificity::Incomparable);
            return Ok(());
        }
        let left: Vec<KType> = shape_slots(a, &types).collect();
        let right: Vec<KType> = shape_slots(b, &types).collect();
        let class = |index: usize| ranking.get(index).map_or(index, |c| usize::from(*c));
        let mut expected = Specificity::Equal;
        for current in 0..left.len() {
            let in_class = || (0..left.len()).filter(|index| class(*index) == current);
            let more = in_class().all(|i| is_subtype_of(&types, scratch, left[i], right[i]));
            let less = in_class().all(|i| is_subtype_of(&types, scratch, right[i], left[i]));
            match (more, less) {
                (true, false) => {
                    expected = Specificity::StrictlyMore;
                    break;
                }
                (false, true) => {
                    expected = Specificity::StrictlyLess;
                    break;
                }
                (true, true) => {}
                (false, false) => expected = Specificity::Incomparable,
            }
        }
        prop_assert_eq!(actual, expected);
    }

    #[test]
    fn a_shape_below_another_admits_what_it_admits(a in shape(), b in shape()) {
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        let comparable = shape_return(a, &types).is_some()
            && shape_return(b, &types).is_some()
            && shape_quantifiers(a, &types).is_empty()
            && shape_quantifiers(b, &types).is_empty()
            && shape_keys_equal(a, b, &types);
        if !comparable || !is_subtype_of(&types, scratch, a, b) {
            return Ok(());
        }
        // `a ≤ b` on monomorphic shapes means every position `b` accepts, `a` accepts too.
        let arity = shape_slots(a, &types).count();
        let world = world();
        let mut runner = proptest::test_runner::TestRunner::deterministic();
        for _ in 0..8 {
            let arguments = arb_arguments(world.clone(), arity)
                .new_tree(&mut runner)
                .expect("the argument strategy produces a tuple")
                .current();
            if admits_tuple(&types, scratch, b, &arguments) {
                prop_assert!(admits_tuple(&types, scratch, a, &arguments));
            }
        }
    }
}

/// Whether `shape` admits one argument tuple at its slot positions.
fn admits_tuple(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    shape: KType,
    arguments: &[KType],
) -> bool {
    let mut collector = Collector::new(scratch, quantifier_bounds(types, shape));
    for (slot, argument) in shape_slots(shape, types).zip(arguments) {
        if admits_with(
            types,
            scratch,
            slot,
            *argument,
            Variance::Co,
            &mut collector,
        )
        .is_err()
        {
            return false;
        }
    }
    collector.solve(types).is_ok()
}

// --- 7. Unification ---

proptest! {
    #![proptest_config(binary())]

    #[test]
    fn admission_without_quantifiers_is_the_order(a in one(), b in one()) {
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        if types.contains_quantified(a) {
            return Ok(());
        }
        let mut collector = Collector::new(scratch, &[]);
        let admitted = admits_with(&types, scratch, a, b, Variance::Co, &mut collector).is_ok();
        prop_assert_eq!(admitted, satisfied_by(&types, scratch, a, b));
    }

    #[test]
    fn a_carried_variable_is_admitted_where_its_bound_is(a in one(), b in one()) {
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        let Some(bound) = types.node(b).rigid_bound() else {
            return Ok(());
        };
        // A generated type holds no free quantifier, so a declared side that has one is built:
        // `a | LIST OF X` reaches the unifier's leaf through the list, and a union bound spanning
        // `a`'s members and the list reaches its member-by-member fallback.
        let variable = types.list(types.quantified(0, KType::ANY));
        for declared in [a, types.union_of(scratch, &[a, variable])] {
            let mut through_bound = Collector::new(scratch, &[KType::ANY]);
            if admits_with(&types, scratch, declared, bound, Variance::Co, &mut through_bound)
                .is_err()
            {
                continue;
            }
            let mut collector = Collector::new(scratch, &[KType::ANY]);
            prop_assert!(
                admits_with(&types, scratch, declared, b, Variance::Co, &mut collector).is_ok(),
                "a position its bound fills refused the variable",
            );
        }
    }

    #[test]
    fn a_solution_is_the_least_instance_of_its_contributions(a in shape(), b in shape()) {
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        let bounds = quantifier_bounds(&types, a);
        if bounds.is_empty() {
            return Ok(());
        }
        let slots: Vec<KType> = shape_slots(a, &types).collect();
        let arguments: Vec<KType> = shape_slots(b, &types).collect();
        if slots.len() != arguments.len() || !shape_keys_equal(a, b, &types) {
            return Ok(());
        }
        let mut collector = Collector::new(scratch, bounds);
        for (slot, argument) in slots.iter().zip(arguments.iter()) {
            if admits_with(&types, scratch, *slot, *argument, Variance::Co, &mut collector).is_err() {
                return Ok(());
            }
        }
        let solution = match collector.solve(&types) {
            Ok(solution) => solution,
            // A solve fails only where the set its pair denotes is empty.
            Err(UnifyFailure::Disagree { lower, upper, .. }) => {
                prop_assert!(!is_subtype_of(&types, scratch, lower, upper));
                return Ok(());
            }
            Err(UnifyFailure::Mismatch) => return Ok(()),
        };
        // Substituting the solution into the declared slots yields positions the arguments fill.
        for (slot, argument) in slots.iter().zip(arguments.iter()) {
            let solved = substitute_quantified(&types, scratch, *slot, &solution);
            prop_assert!(satisfied_by(&types, scratch, solved, *argument));
        }
        // Admission does not depend on the order the slots are read.
        let mut backwards = Collector::new(scratch, bounds);
        for (slot, argument) in slots.iter().zip(arguments.iter()).rev() {
            prop_assert!(
                admits_with(&types, scratch, *slot, *argument, Variance::Co, &mut backwards).is_ok()
            );
        }
        let again = backwards.solve(&types).ok();
        prop_assert_eq!(again.as_deref(), Some(&solution[..]));

        for (index, solved) in solution.iter().enumerate() {
            let (lower, upper) = collector.contributions(index);
            let bound = collector.bound(index);
            let equivalent = |x: KType, y: KType| {
                is_subtype_of(&types, scratch, x, y) && is_subtype_of(&types, scratch, y, x)
            };
            // The least instance of the pair: its lower end where a lower contribution reached the
            // variable, and its upper end otherwise. An end is the extremum of its contributions
            // where they have one — up to equivalence, since two binders may be equivalent handles.
            if !lower.is_empty() {
                prop_assert_eq!(*solved, join_iter(&types, scratch, lower.iter().copied()));
                for ceiling in upper.iter().chain([&bound]) {
                    prop_assert!(is_subtype_of(&types, scratch, *solved, *ceiling));
                }
                let maximum = lower.iter().find(|candidate| {
                    lower.iter().all(|other| is_subtype_of(&types, scratch, *other, **candidate))
                });
                if let Some(maximum) = maximum {
                    prop_assert!(equivalent(*solved, *maximum));
                }
            } else {
                let met = upper
                    .iter()
                    .fold(bound, |met, each| meet(&types, scratch, met, *each));
                prop_assert_eq!(*solved, met);
                let minimum = upper.iter().find(|candidate| {
                    upper.iter().chain([&bound]).all(|other| {
                        is_subtype_of(&types, scratch, **candidate, *other)
                    })
                });
                if let Some(minimum) = minimum {
                    prop_assert!(equivalent(*solved, *minimum));
                }
            }
            // Every solution lies under its variable's declared bound.
            prop_assert!(is_subtype_of(&types, scratch, *solved, bound));
        }
    }
}

/// What one case draws: per slot, where its static type comes from and its mode (within it,
/// exactly it, or bounded below); the argument pool and the types a carried argument is met with;
/// and per lexical level, the type the run binds it to.
#[derive(Clone, Debug)]
struct Draw {
    picks: Vec<(u8, u8)>,
    pool: Vec<KType>,
    drawn: Vec<KType>,
    instances: Vec<u8>,
}

/// A [`Draw`], weighted toward the candidate's own slot so the static solve succeeds often enough
/// for the laws to bite.
fn draw() -> impl Strategy<Value = Draw> {
    let source = prop_oneof![3 => Just(0u8), 1 => Just(1u8), 1 => Just(2u8)];
    (
        prop::collection::vec((source, 0u8..3), 4),
        arb_arguments(world(), 4),
        arb_arguments(world(), 4),
        prop::collection::vec(0u8..3, 2),
    )
        .prop_map(|(picks, pool, drawn, instances)| Draw {
            picks,
            pool,
            drawn,
            instances,
        })
}

/// `kt` as the run carries it: each lexical variable replaced by the type the run binds its level
/// to — its bound, its lower end, or the bound met with a pool type and joined with the lower end,
/// as `draw` says.
fn instance(types: &TypeRegistry<'_>, scratch: BumpAllocator<'_>, kt: KType, draw: &Draw) -> KType {
    read_through(types, scratch, kt, Side::Above, &mut |node| match *node {
        TypeNode::Lexical {
            level,
            lower,
            bound,
            ..
        } => Some(Interval::point(
            match draw.instances.get(level).copied().unwrap_or(0) {
                0 => bound,
                1 => lower,
                _ => join(
                    types,
                    scratch,
                    lower,
                    meet(types, scratch, bound, draw.pool[level % draw.pool.len()]),
                ),
            },
        )),
        _ => None,
    })
}

/// Static arguments for `a`, one per slot, beside carried arguments within them. Slot `k`'s static
/// type is drawn by `draw.picks[k]` from `a`'s own slot with its group erased, from one of `b`'s so
/// erased, or from the argument pool. Its mode is the pick's second element: `1` is exactly the
/// static type, carried as the run carries it; `2`, where the static type holds no rigid variable
/// and meets a drawn type above `Never`, is bounded below by that meet, which it carries; anything
/// else is within the static type, carrying it met with a drawn type. `None` where some argument is
/// `Never`.
fn static_and_carried(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    a: KType,
    b: KType,
    draw: &Draw,
) -> Option<(Vec<Interval>, Vec<KType>)> {
    let own: Vec<KType> = shape_slots(erase_quantified(types, scratch, a), types).collect();
    let other: Vec<KType> = shape_slots(erase_quantified(types, scratch, b), types).collect();
    let mut arguments = Vec::with_capacity(own.len());
    let mut carried = Vec::with_capacity(own.len());
    for (k, (source, mode)) in draw.picks.iter().take(own.len()).enumerate() {
        let static_type = match source {
            0 => own[k],
            1 if !other.is_empty() => other[k % other.len()],
            _ => draw.pool[k],
        };
        let run = instance(types, scratch, static_type, draw);
        let below = meet(types, scratch, static_type, draw.drawn[k]);
        let (argument, one) = match mode {
            1 => (Interval::point(static_type), run),
            2 if !types.contains_rigid(static_type) && below != KType::NEVER => (
                Interval {
                    lower: below,
                    upper: static_type,
                },
                below,
            ),
            _ => (
                Interval::within(static_type),
                meet(types, scratch, run, draw.drawn[k]),
            ),
        };
        if argument.upper == KType::NEVER || one == KType::NEVER {
            return None;
        }
        arguments.push(argument);
        carried.push(one);
    }
    Some((arguments, carried))
}

/// `interval` with each end as the run carries it.
fn carried_interval(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    interval: Interval,
    draw: &Draw,
) -> Interval {
    Interval {
        lower: instance(types, scratch, interval.lower, draw),
        upper: instance(types, scratch, interval.upper, draw),
    }
}

/// Whether `kt` lies within `interval`.
fn within(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    kt: KType,
    interval: Interval,
) -> bool {
    is_subtype_of(types, scratch, interval.lower, kt)
        && is_subtype_of(types, scratch, kt, interval.upper)
}

/// `shape`'s group solved jointly over one argument per slot, or `None`.
fn solve_jointly<'s>(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'s>,
    shape: KType,
    arguments: &[KType],
) -> Option<BumpVec<'s, KType>> {
    let mut collector = Collector::new(scratch, quantifier_bounds(types, shape));
    for (slot, argument) in shape_slots(shape, types).zip(arguments) {
        admits_with(
            types,
            scratch,
            slot,
            *argument,
            Variance::Co,
            &mut collector,
        )
        .ok()?;
    }
    collector.solve(types).ok()
}

proptest! {
    #![proptest_config(binary())]

    /// For carried types each within its static interval, every solution over the carried types
    /// lies in the interval reported over the static ones.
    #[test]
    fn a_carried_solution_lies_in_its_static_interval(
        a in shape(),
        b in shape(),
        draw in draw(),
    ) {
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        let Some((arguments, carried)) = static_and_carried(&types, scratch, a, b, &draw) else {
            return Ok(());
        };
        let run_interval = |interval| carried_interval(&types, scratch, interval, &draw);
        let uppers: Vec<KType> = arguments.iter().map(|argument| argument.upper).collect();
        let (Some(statics), Some(solution)) = (
            solve_jointly(&types, scratch, a, &uppers),
            solve_jointly(&types, scratch, instance(&types, scratch, a, &draw), &carried),
        ) else {
            return Ok(());
        };
        let slots: Vec<KType> = shape_slots(a, &types).collect();
        // A static type holding a lexical variable is solved through its bound, where the call
        // solves through the type the run binds it to.
        let all_exact = arguments
            .iter()
            .all(|argument| argument.is_exact() && !types.contains_rigid(argument.upper));
        let reported = intervals(
            &types,
            scratch,
            &slots,
            quantifier_bounds(&types, a),
            &statics,
            all_exact,
        );
        for (solved, interval) in solution.iter().zip(reported.iter()) {
            prop_assert!(
                within(&types, scratch, *solved, run_interval(*interval)),
                "a carried solution left its static interval",
            );
        }
    }

    /// A verdict holds of every call within the static types: *always* admits, *never* does not,
    /// and a solution lies in the intervals judging reported.
    #[test]
    fn a_verdict_holds_of_every_call_within_its_static_types(
        a in shape(),
        b in shape(),
        draw in draw(),
    ) {
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        let Some((arguments, carried)) = static_and_carried(&types, scratch, a, b, &draw) else {
            return Ok(());
        };
        let run_interval = |interval| carried_interval(&types, scratch, interval, &draw);
        let judged = judge_by_class(&types, scratch, a, &arguments);
        // The run binds the candidate's lexical variables as it binds its arguments'.
        let run_shape = instance(&types, scratch, a, &draw);
        let admitted = admit_by_class(&types, scratch, run_shape, &carried);
        match judged.verdict {
            Verdict::Always => prop_assert!(admitted.is_some(), "an always candidate refused"),
            Verdict::Never => prop_assert!(admitted.is_none(), "a never candidate admitted"),
            Verdict::Maybe => {}
        }
        if let (Some(reported), Some(solution)) = (judged.intervals, admitted) {
            for (solved, interval) in solution.iter().zip(reported) {
                prop_assert!(
                    within(&types, scratch, *solved, run_interval(*interval)),
                    "a class-by-class solution left its judged interval",
                );
            }
        }
    }
}

// --- 8. Sealing ---

proptest! {
    #![proptest_config(binary())]

    #[test]
    fn sealing_is_order_insensitive_and_idempotent(reprs in prop::collection::vec(small(), 1..4)) {
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        let world = world();
        let names: Vec<_> = world.type_names[..reprs.len().min(3)].to_vec();
        let reprs = &reprs[..names.len()];
        let forwards = seal(&types, scratch, &names, reprs, false);
        let backwards = seal(&types, scratch, &names, reprs, true);
        // The same group presented in either member order seals to the same handles.
        let mut mirrored = backwards.clone();
        mirrored.reverse();
        prop_assert_eq!(forwards.clone(), mirrored);
        // Re-presenting the same group yields the same handles.
        prop_assert_eq!(forwards, seal(&types, scratch, &names, reprs, false));
    }
}

/// Seal a group of newtypes over `reprs`, each also referencing its successor as a sibling, in
/// announcement order or reversed, over a window hosted in `scratch`. Returns the member handles in
/// announcement order.
fn seal(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    names: &[TypeSymbol],
    reprs: &[KType],
    reversed: bool,
) -> Vec<KType> {
    let order: Vec<usize> = if reversed {
        (0..names.len()).rev().collect()
    } else {
        (0..names.len()).collect()
    };
    let announced: Vec<(TypeSymbol, KKind)> = order
        .iter()
        .map(|index| (names[*index], KKind::NewType))
        .collect();
    let window = RecursiveGroupWindow::new(scratch, &announced);
    for (position, index) in order.iter().enumerate() {
        let successor = window.sibling(names[(index + 1) % names.len()], KKind::NewType, types);
        let body = types.union_of(scratch, &[reprs[*index], successor]);
        window.fill_member(position, RelativeSchema::NewType(body), types, scratch);
    }
    let sealed = window.sealed().expect("the window seals on its last fill");
    (0..names.len())
        .map(|index| sealed.member(index).expect("every announced member sealed"))
        .collect()
}

// --- 9. Signatures ---

proptest! {
    #![proptest_config(binary())]

    #[test]
    fn schema_relations_bound_their_operands(a in one(), b in one()) {
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        let read = |kt: KType| match types.node(kt) {
            TypeNode::Signature { schema, .. } => Some(schema),
            _ => None,
        };
        let (Some(left), Some(right)) = (read(a), read(b)) else {
            return Ok(());
        };
        prop_assert!(sig_subtype(&types, scratch, left, left).is_ok());
        prop_assert!(sig_subtype(&types, scratch, left, SigSchema::EMPTY).is_ok());
        if let Some(met) = meet_schemas(&types, scratch, left, right) {
            let met = read(met).expect("a meet of two schemas interns as a signature");
            prop_assert!(sig_subtype(&types, scratch, met, left).is_ok());
            prop_assert!(sig_subtype(&types, scratch, met, right).is_ok());
        }
    }

    /// Every interned schema is stored in its canonical order — the invariant that lets every
    /// reader walk a table straight through and look a name up by binary search — and
    /// specializing one by nothing names it again.
    #[test]
    fn a_schema_is_stored_in_canonical_order(a in one()) {
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        let TypeNode::Signature { schema, .. } = types.node(a) else {
            return Ok(());
        };
        prop_assert!(schema.abstract_members.windows(2).all(|w| w[0].0 < w[1].0));
        prop_assert!(schema.manifest_members.windows(2).all(|w| w[0].0 < w[1].0));
        prop_assert!(schema.value_slots.windows(2).all(|w| w[0].0 < w[1].0));
        prop_assert!(
            schema
                .operators
                .iter()
                .all(|group| group.members.windows(2).all(|w| w[0] < w[1]))
        );
        prop_assert_eq!(specialize_schema(&types, scratch, schema, &[]), a);
    }
}

// --- 10. Rendering ---

proptest! {
    #![proptest_config(binary())]

    #[test]
    fn rendering_is_total_and_deterministic(a in one()) {
        let world = world();
        let once = crate::type_lattice::display_name(a, &world.types, &world.symbols).to_string();
        let twice = crate::type_lattice::display_name(a, &world.types, &world.symbols).to_string();
        prop_assert!(!once.is_empty());
        prop_assert_eq!(once, twice);
    }

    #[test]
    fn a_canonical_overload_set_is_an_antichain(
        overloads in prop::collection::vec(shape(), 1..4)
    ) {
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        let mut kept = BumpVec::with_capacity_in(overloads.len(), scratch);
        kept.extend_from_slice(&overloads);
        canonical_overloads(&types, scratch, &mut kept);
        // Canonical by subsumption, the same rule `union_of` applies to a union's members: no
        // survivor lies below another, so no member promises only what a sibling already does.
        for (index, one) in kept.iter().enumerate() {
            for (peer, other) in kept.iter().enumerate() {
                prop_assert!(
                    index == peer || !is_subtype_of(&types, scratch, *other, *one),
                    "a canonical overload set kept a member below another",
                );
            }
        }
        // And the drop is complete: everything dropped has a survivor standing for it, so the
        // canonical set promises everything the input did.
        for dropped in &overloads {
            prop_assert!(
                kept.contains(dropped)
                    || kept
                        .iter()
                        .any(|survivor| is_subtype_of(&types, scratch, *survivor, *dropped)),
                "an overload was dropped with no survivor below it",
            );
        }
    }
}
