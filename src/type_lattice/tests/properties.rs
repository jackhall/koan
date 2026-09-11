//! The laws.
//!
//! Every statement here is a property of the algebra, not a pin on a particular input: a rewrite of
//! any walk has these as its oracle. One registry per test thread, so handles generated inside one
//! law all name content the same table holds.

use proptest::prelude::*;
use proptest::strategy::ValueTree;

use crate::type_lattice::handle::KType;
use crate::type_lattice::lattice::{join, meet};
use crate::type_lattice::node::TypeNode;
use crate::type_lattice::order::{is_more_specific_than, is_subtype_of, satisfied_by};
use crate::type_lattice::registry::TypeRegistry;
use crate::type_lattice::schema::{
    SigSchema, shape_keys_equal, shape_quantifiers, shape_return, shape_slots,
};
use crate::type_lattice::shape::Specificity;
use crate::type_lattice::sig_relations::{
    admits_slots, meet_schemas, shape_specificity, sig_subtype,
};
use crate::type_lattice::substitute::{
    canonicalize_binder, erase_quantified, instantiate_quantified, quantifier_bounds,
    slot_more_specific_or_equal, slot_satisfied_by, slot_types_equal, substitute_quantified,
    substitute_sig_members,
};
use crate::type_lattice::unify::{Collector, admits_with};
use crate::type_lattice::walk::Variance;
use crate::type_lattice::window::{RecursiveGroupWindow, RelativeSchema, SealedGroup};

use super::generators::{World, arb_arguments, arb_shape_type, arb_type};

thread_local! {
    /// One live registry and one alphabet per test thread — proptest runs each `#[test]` on its
    /// own, so a law's generated handles all name content one table holds.
    static WORLD: World = World::new();
}

fn world() -> World {
    WORLD.with(|world| world.clone())
}

fn registry() -> std::rc::Rc<TypeRegistry> {
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

fn binary() -> ProptestConfig {
    ProptestConfig {
        cases: 256,
        ..ProptestConfig::default()
    }
}

fn ternary() -> ProptestConfig {
    ProptestConfig {
        cases: 96,
        ..ProptestConfig::default()
    }
}

// --- 1. Join and meet ---

proptest! {
    #![proptest_config(binary())]

    #[test]
    fn join_and_meet_are_commutative_and_idempotent(a in one(), b in one()) {
        let types = registry();
        prop_assert_eq!(join(&types, a, b), join(&types, b, a));
        prop_assert_eq!(meet(&types, a, b), meet(&types, b, a));
        prop_assert_eq!(join(&types, a, a), a);
        prop_assert_eq!(meet(&types, a, a), a);
    }

    #[test]
    fn join_and_meet_absorb_each_other(a in one(), b in one()) {
        let types = registry();
        prop_assert_eq!(join(&types, a, meet(&types, a, b)), a);
        prop_assert_eq!(meet(&types, a, join(&types, a, b)), a);
    }

    #[test]
    fn never_and_any_are_the_identities(a in one()) {
        let types = registry();
        prop_assert_eq!(join(&types, a, KType::NEVER), a);
        prop_assert_eq!(meet(&types, a, KType::ANY), a);
    }
}

proptest! {
    #![proptest_config(ternary())]

    #[test]
    fn join_and_meet_are_associative(a in small(), b in small(), c in small()) {
        let types = registry();
        prop_assert_eq!(
            join(&types, join(&types, a, b), c),
            join(&types, a, join(&types, b, c))
        );
        prop_assert_eq!(
            meet(&types, meet(&types, a, b), c),
            meet(&types, a, meet(&types, b, c))
        );
    }
}

// --- 2. The order ---

proptest! {
    #![proptest_config(binary())]

    #[test]
    fn the_order_is_reflexive_and_bounded(a in one()) {
        let types = registry();
        prop_assert!(is_subtype_of(&types, a, a));
        prop_assert!(is_subtype_of(&types, KType::NEVER, a));
        prop_assert!(is_subtype_of(&types, a, KType::ANY));
        prop_assert!(!is_more_specific_than(&types, a, a));
    }

    #[test]
    fn the_order_is_antisymmetric(a in one(), b in one()) {
        let types = registry();
        if is_subtype_of(&types, a, b) && is_subtype_of(&types, b, a) {
            prop_assert_eq!(a, b);
        }
    }

    #[test]
    fn the_order_agrees_with_join_and_meet(a in one(), b in one()) {
        let types = registry();
        let below = is_subtype_of(&types, a, b);
        prop_assert_eq!(below, join(&types, a, b) == b);
        prop_assert_eq!(below, meet(&types, a, b) == a);
        prop_assert_eq!(below, satisfied_by(&types, b, a));
        prop_assert_eq!(is_more_specific_than(&types, a, b), a != b && below);
    }

    #[test]
    fn a_rigid_variable_has_only_itself_and_never_below_it(a in one(), b in one()) {
        let types = registry();
        let rigid = types.with_node(b, |node| {
            matches!(
                node,
                TypeNode::Quantified { .. } | TypeNode::AbstractType { .. }
            )
        });
        if rigid {
            prop_assert_eq!(
                is_subtype_of(&types, a, b),
                a == b || a == KType::NEVER
            );
        }
    }
}

proptest! {
    #![proptest_config(ternary())]

    #[test]
    fn the_order_is_transitive(a in small(), b in small(), c in small()) {
        let types = registry();
        if is_subtype_of(&types, a, b) && is_subtype_of(&types, b, c) {
            prop_assert!(is_subtype_of(&types, a, c));
        }
    }
}

// --- 3. Unions ---

proptest! {
    #![proptest_config(binary())]

    #[test]
    fn union_of_is_canonical(members in prop::collection::vec(one(), 1..5)) {
        let types = registry();
        let union = types.union_of(&members);
        let mut reversed = members.clone();
        reversed.reverse();
        prop_assert_eq!(union, types.union_of(&reversed));
        prop_assert_eq!(union, types.union_of(&[union]));
        // Flattening: wrapping the result in another union changes nothing.
        let mut nested = members.clone();
        nested.push(union);
        prop_assert_eq!(union, types.union_of(&nested));
        // Never is dropped and `Any` absorbs.
        let mut with_never = members.clone();
        with_never.push(KType::NEVER);
        prop_assert_eq!(union, types.union_of(&with_never));
        let mut with_any = members.clone();
        with_any.push(KType::ANY);
        prop_assert_eq!(KType::ANY, types.union_of(&with_any));
        // No member of a canonical union lies below another.
        types.with_node(union, |node| {
            if let TypeNode::Union { members } = node {
                for (index, member) in members.iter().enumerate() {
                    for (peer, other) in members.iter().enumerate() {
                        assert!(
                            index == peer || !is_subtype_of(&types, *member, *other),
                            "a canonical union kept a member below another",
                        );
                    }
                }
            }
        });
        // Every member is below the union, and the union is below anything all members are below.
        for member in &members {
            prop_assert!(is_subtype_of(&types, *member, union));
        }
    }
}

// --- 4. Interning ---

proptest! {
    #![proptest_config(binary())]

    #[test]
    fn equal_content_interns_once(a in one(), b in one()) {
        let types = registry();
        prop_assert_eq!(types.list(a), types.list(a));
        prop_assert_eq!(types.dict(a, b), types.dict(a, b));
        // A node read back out and re-interned names the same handle.
        prop_assert_eq!(types.intern(types.node(a)), a);
        prop_assert_eq!(types.intern(types.node(b)), b);
    }
}

// --- 5. Substitution ---

proptest! {
    #![proptest_config(binary())]

    #[test]
    fn substitution_of_nothing_is_the_identity(a in one(), b in one()) {
        let types = registry();
        prop_assert_eq!(substitute_quantified(&types, a, &[]), a);
        if !types.contains_quantified(a) {
            prop_assert_eq!(substitute_quantified(&types, a, &[b, b, b]), a);
        }
        let empty = crate::type_lattice::schema::TypeMemberMap::default();
        prop_assert_eq!(
            substitute_sig_members(&types, a, crate::memory::ScopeId::SENTINEL, &empty),
            a
        );
    }

    #[test]
    fn erasing_is_instantiating_at_the_bounds(a in one()) {
        let types = registry();
        let bounds = quantifier_bounds(&types, a);
        prop_assert_eq!(
            erase_quantified(&types, a),
            instantiate_quantified(&types, a, &bounds)
        );
    }

    #[test]
    fn canonicalizing_a_binder_is_idempotent(a in one()) {
        let types = registry();
        let once = canonicalize_binder(&types, a, crate::memory::ScopeId::SENTINEL);
        prop_assert_eq!(
            canonicalize_binder(&types, once, crate::memory::ScopeId::SENTINEL),
            once
        );
    }

    #[test]
    fn the_slot_relations_are_their_definitions(a in one(), b in one(), c in one()) {
        let types = registry();
        let world = world();
        let mut members = crate::type_lattice::schema::TypeMemberMap::default();
        members.insert(world.type_names[0], c);
        let id = crate::memory::ScopeId::SENTINEL;
        let substituted = substitute_sig_members(&types, a, id, &members);
        prop_assert_eq!(
            slot_satisfied_by(&types, a, b, id, &members),
            satisfied_by(&types, substituted, b)
        );
        prop_assert_eq!(
            slot_more_specific_or_equal(&types, a, b, id, &members),
            is_subtype_of(&types, substituted, b)
        );
        prop_assert_eq!(
            slot_types_equal(&types, a, b, id, &members),
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
        let read = types.with_node(a, |node| match node {
            TypeNode::ExpressionShape {
                quantifiers,
                elements,
                ret,
            } => Some((quantifiers.clone(), elements.to_vec(), *ret)),
            _ => None,
        });
        if let Some((quantifiers, elements, ret)) = read {
            let again = types.shape_type(&quantifiers, &elements, ret);
            prop_assert_eq!(again.handle, a);
        }
    }

    #[test]
    fn specificity_flips_when_its_arguments_swap(a in shape(), b in shape()) {
        let types = registry();
        prop_assert_eq!(
            shape_specificity(&types, a, a),
            Specificity::Equal,
            "a shape is not `Equal` to itself"
        );
        let forward = shape_specificity(&types, a, b);
        let backward = shape_specificity(&types, b, a);
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
        // Two things that are not both shapes share no bucket to rank under. The empty element run
        // a non-shape reads as would otherwise make every pair of leaves compare `Equal`.
        if !crate::type_lattice::schema::is_shape(a, &types) {
            prop_assert_eq!(shape_specificity(&types, a, b), Specificity::Incomparable);
            prop_assert_eq!(shape_specificity(&types, b, a), Specificity::Incomparable);
            prop_assert!(!admits_slots(a, b, &types));
            prop_assert!(!admits_slots(b, a, &types));
        }
    }

    #[test]
    fn monomorphic_specificity_is_the_pointwise_fold(a in shape(), b in shape()) {
        let types = registry();
        let monomorphic = shape_quantifiers(a, &types).is_empty()
            && shape_quantifiers(b, &types).is_empty()
            && shape_return(a, &types).is_some()
            && shape_return(b, &types).is_some()
            && shape_keys_equal(a, b, &types);
        if !monomorphic {
            return Ok(());
        }
        let (left, right) = (shape_slots(a, &types), shape_slots(b, &types));
        let more = left
            .iter()
            .zip(right.iter())
            .all(|(x, y)| is_subtype_of(&types, *x, *y));
        let less = left
            .iter()
            .zip(right.iter())
            .all(|(x, y)| is_subtype_of(&types, *y, *x));
        let expected = match (more, less) {
            (true, false) => Specificity::StrictlyMore,
            (false, true) => Specificity::StrictlyLess,
            (true, true) => Specificity::Equal,
            (false, false) => Specificity::Incomparable,
        };
        prop_assert_eq!(shape_specificity(&types, a, b), expected);
    }

    #[test]
    fn a_shape_below_another_admits_what_it_admits(a in shape(), b in shape()) {
        let types = registry();
        let comparable = shape_return(a, &types).is_some()
            && shape_return(b, &types).is_some()
            && shape_quantifiers(a, &types).is_empty()
            && shape_quantifiers(b, &types).is_empty()
            && shape_keys_equal(a, b, &types);
        if !comparable || !is_subtype_of(&types, a, b) {
            return Ok(());
        }
        // `a ≤ b` on monomorphic shapes means every position `b` accepts, `a` accepts too.
        let arity = shape_slots(a, &types).len();
        let world = world();
        let mut runner = proptest::test_runner::TestRunner::deterministic();
        for _ in 0..8 {
            let arguments = arb_arguments(world.clone(), arity)
                .new_tree(&mut runner)
                .expect("the argument strategy produces a tuple")
                .current();
            if admits_tuple(&types, b, &arguments) {
                prop_assert!(admits_tuple(&types, a, &arguments));
            }
        }
    }
}

/// Whether `shape` admits one argument tuple at its slot positions.
fn admits_tuple(types: &TypeRegistry, shape: KType, arguments: &[KType]) -> bool {
    let slots = shape_slots(shape, types);
    let mut collector = Collector::new(shape_quantifiers(shape, types).len());
    for (slot, argument) in slots.iter().zip(arguments.iter()) {
        if admits_with(types, *slot, *argument, Variance::Co, &mut collector).is_err() {
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
        if types.contains_quantified(a) {
            return Ok(());
        }
        let mut collector = Collector::default();
        let admitted = admits_with(&types, a, b, Variance::Co, &mut collector).is_ok();
        prop_assert_eq!(admitted, satisfied_by(&types, a, b));
    }

    #[test]
    fn a_solution_is_the_extremum_of_its_contributions(a in shape(), b in shape()) {
        let types = registry();
        let bounds = quantifier_bounds(&types, a);
        if bounds.is_empty() {
            return Ok(());
        }
        let slots = shape_slots(a, &types);
        let arguments = shape_slots(b, &types);
        if slots.len() != arguments.len() || !shape_keys_equal(a, b, &types) {
            return Ok(());
        }
        let mut collector = Collector::new(bounds.len());
        for (slot, argument) in slots.iter().zip(arguments.iter()) {
            if admits_with(&types, *slot, *argument, Variance::Co, &mut collector).is_err() {
                return Ok(());
            }
        }
        let Ok(solution) = collector.solve(&types) else {
            return Ok(());
        };
        // Substituting the solution into the declared slots yields positions the arguments fill.
        for (slot, argument) in slots.iter().zip(arguments.iter()) {
            let solved = substitute_quantified(&types, *slot, &solution);
            prop_assert!(satisfied_by(&types, solved, *argument));
        }
        // Admission does not depend on the order the slots are read.
        let mut backwards = Collector::new(bounds.len());
        for (slot, argument) in slots.iter().zip(arguments.iter()).rev() {
            prop_assert!(
                admits_with(&types, *slot, *argument, Variance::Co, &mut backwards).is_ok()
            );
        }
        prop_assert_eq!(backwards.solve(&types).ok(), Some(solution.clone()));

        for (index, solved) in solution.iter().enumerate() {
            let (lower, upper) = collector.contributions(index);
            let bound = collector.bound(index);
            // The solution is always a contribution or the declared bound — never a type the
            // arguments and the declaration did not already spell between them.
            prop_assert!(
                lower.contains(solved) || upper.contains(solved) || *solved == bound,
                "the solver minted a type nobody wrote",
            );
            // And it is the extremum of the set that constrains it: the maximum of the lower
            // contributions where there are any, else the minimum of the upper ones. That is what
            // "lies under every other solution that also admits" means for a lower-constrained
            // variable — every admitting alternative is above every lower contribution, and the
            // solution *is* one of them.
            if !lower.is_empty() {
                prop_assert!(lower.contains(solved));
                for contribution in lower {
                    prop_assert!(is_subtype_of(&types, *contribution, *solved));
                }
            } else if !upper.is_empty() {
                prop_assert!(upper.contains(solved));
                for contribution in upper {
                    prop_assert!(is_subtype_of(&types, *solved, *contribution));
                }
            } else {
                prop_assert_eq!(*solved, bound);
            }
            // Every solution lies under its variable's declared bound.
            prop_assert!(is_subtype_of(&types, *solved, bound));
        }
    }
}

// --- 8. Sealing ---

proptest! {
    #![proptest_config(binary())]

    #[test]
    fn sealing_is_order_insensitive_and_idempotent(reprs in prop::collection::vec(small(), 1..4)) {
        let types = registry();
        let world = world();
        let names: Vec<_> = world.type_names[..reprs.len().min(3)].to_vec();
        let reprs = &reprs[..names.len()];
        let forwards = seal(&types, &names, reprs, false);
        let backwards = seal(&types, &names, reprs, true);
        // The same group presented in either member order seals to the same handles.
        let mut mirrored = backwards.members.clone();
        mirrored.reverse();
        prop_assert_eq!(forwards.members.clone(), mirrored);
        // Re-presenting the same group yields the same handles.
        prop_assert_eq!(forwards.members.clone(), seal(&types, &names, reprs, false).members);
    }
}

/// Seal a group of newtypes over `reprs`, each also referencing its successor as a sibling, in
/// announcement order or reversed.
fn seal(
    types: &TypeRegistry,
    names: &[crate::parse::TypeSymbol],
    reprs: &[KType],
    reversed: bool,
) -> SealedGroup {
    let order: Vec<usize> = if reversed {
        (0..names.len()).rev().collect()
    } else {
        (0..names.len()).collect()
    };
    let announced: Vec<_> = order
        .iter()
        .map(|index| (names[*index], crate::type_lattice::kind::KKind::NewType))
        .collect();
    let window = RecursiveGroupWindow::new(announced);
    let mut sealed = None;
    for (position, index) in order.iter().enumerate() {
        let successor = window.sibling(
            names[(index + 1) % names.len()],
            crate::type_lattice::kind::KKind::NewType,
            types,
        );
        let body = types.union_of(&[reprs[*index], successor]);
        sealed = window.fill_member(position, RelativeSchema::NewType(body), types);
    }
    sealed.expect("the window seals on its last fill")
}

// --- 9. Signatures ---

proptest! {
    #![proptest_config(binary())]

    #[test]
    fn schema_relations_bound_their_operands(a in one(), b in one()) {
        let types = registry();
        let read = |kt: KType| {
            types.with_node(kt, |node| match node {
                TypeNode::Signature { schema, .. } => Some(schema.clone()),
                _ => None,
            })
        };
        let (Some(left), Some(right)) = (read(a), read(b)) else {
            return Ok(());
        };
        prop_assert!(sig_subtype(&types, &left, &left).is_ok());
        prop_assert!(sig_subtype(&types, &left, &SigSchema::empty()).is_ok());
        if let Some(met) = meet_schemas(&types, &left, &right) {
            prop_assert!(sig_subtype(&types, &met, &left).is_ok());
            prop_assert!(sig_subtype(&types, &met, &right).is_ok());
        }
    }
}

// --- 10. Rendering ---

proptest! {
    #![proptest_config(binary())]

    #[test]
    fn rendering_is_total_and_deterministic(a in one()) {
        let world = world();
        let once = crate::type_lattice::render::name(a, &world.types, &world.labels);
        let twice = crate::type_lattice::render::name(a, &world.types, &world.labels);
        prop_assert_eq!(once, twice);
        prop_assert!(!crate::type_lattice::render::name(a, &world.types, &world.labels).is_empty());
    }

    #[test]
    fn a_canonical_overload_set_is_an_antichain(
        overloads in prop::collection::vec(shape(), 1..4)
    ) {
        let types = registry();
        let kept =
            crate::type_lattice::schema::canonical_overloads(overloads.clone(), &types);
        // Canonical by subsumption, the same rule `union_of` applies to a union's members: no
        // survivor lies below another, so no member promises only what a sibling already does.
        for (index, one) in kept.iter().enumerate() {
            for (peer, other) in kept.iter().enumerate() {
                prop_assert!(
                    index == peer || !is_subtype_of(&types, *other, *one),
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
                        .any(|survivor| is_subtype_of(&types, *survivor, *dropped)),
                "an overload was dropped with no survivor below it",
            );
        }
    }
}
