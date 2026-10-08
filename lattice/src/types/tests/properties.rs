//! The laws.
//!
//! Every statement here is a property of the algebra, not a pin on a particular input: a rewrite of
//! any walk has these as its oracle. One registry per test thread, so handles generated inside one
//! law all name content the same table holds; each case brings its own scratch region, dropped with
//! the case.
//!
//! The order, join and meet are laws over concrete types, stated through the typed relations;
//! *fits* is stated over every generated type. The laws about the lattice's own machinery — the
//! unifier, ranking, substitution, interning — read their draws raw, as the lattice does.
//!
//! A law draws a case that meets its precondition by construction ([`super::generators`]); an
//! assertion a generator guarantees says so in its message. What remains is a `prop_assume!`, and
//! each config caps the rejects, so a law whose draws drift off their precondition fails loudly.

use proptest::prelude::*;
use proptest::strategy::ValueTree;

use crate::bump::{Bump, BumpAllocator, BumpVec};
use crate::symbols::TypeSymbol;
use crate::types::handle::{DeclaredType, Handle, KType, Parametric, TypeHandle};
use crate::types::kind::KKind;
use crate::types::node::{TypeNode, Variable};
use crate::types::ranking::{Verdict, admit_by_class, judge_by_class, solving_slots};
use crate::types::registry::TypeRegistry;
use crate::types::schema::{Members, canonical_overloads};
use crate::types::shape::{Shape, Specificity, keys_equal, shape_classes, shape_slots};
use crate::types::sig_relations::InstanceFailure;
use crate::types::sig_relations::{admits_shape, shape_specificity, sig_fits};
use crate::types::signatures::{applications, applications_under, is_signature_type};
use crate::types::substitute::{
    Side, bound_above, quantifier_bounds, read_through, substitute_parameters,
    substitute_quantified,
};
use crate::types::typed::{
    fits, instance_under, instantiate_quantified, is_subtype_of, join, meet, satisfied_by,
};
use crate::types::unify::{
    Collector, Interval, UnifyFailure, admits, intervals, most_determined_first, ties,
};
use crate::types::walk::Variance;
use crate::types::walk::unary::{Visit, visit, visit_free_quantified};
use crate::types::window::{RecursiveGroupWindow, RelativeSchema};
use crate::types::{lattice, order};

use super::generators::{
    Groups, Vocabulary, World, arb_any, arb_any_with, arb_argument_pair, arb_arguments,
    arb_bounded_head_chain, arb_chain, arb_concrete, arb_fits_chain, arb_function_type,
    arb_instance_chain, arb_lexical, arb_opaque, arb_ordered_pair, arb_over_head_parameter,
    arb_own_instance, arb_shape_below, arb_shape_pair, arb_shape_type, arb_signature,
    arb_signature_type, arb_wanted_instance,
};

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
fn one() -> BoxedStrategy<DeclaredType<Parametric>> {
    arb_any(world(), 3)
}

/// A generated concrete type at the standard depth: what the order, join and meet relate.
fn concrete() -> BoxedStrategy<KType> {
    arb_concrete(world(), 3)
}

/// A shallower concrete one, for the ternary laws over the concrete lattice.
fn small_concrete() -> BoxedStrategy<KType> {
    arb_concrete(world(), 2)
}

/// A generated expression shape. The laws whose subject is a shape draw from here rather than from
/// the whole vocabulary, where most draws would satisfy them vacuously.
fn shape() -> BoxedStrategy<DeclaredType<Parametric>> {
    arb_shape_type(world(), 3)
}

/// A generated signature type, for the laws whose subject is one.
fn signature_type() -> BoxedStrategy<KType> {
    arb_signature_type(world(), 3)
}

/// A generated function type, for the same reason [`shape`] exists: the laws about a binder's
/// group have nothing to say about a draw that is not one.
fn function() -> BoxedStrategy<DeclaredType<Parametric>> {
    arb_function_type(world(), 3)
}

/// A binder over a group of its own, over anything, nested binders included.
const BINDER: Vocabulary = Vocabulary {
    groups: Groups::Always,
    ..Vocabulary::ANY
};

/// A binder whose only variables are its own group's, and which nests no binder: its instance at
/// concrete bindings is concrete.
const CLOSED_BINDER: Vocabulary = Vocabulary {
    variables: false,
    groups: Groups::Outer,
};

/// A binder that may name lexical variables, and which nests no binder: its instance is concrete
/// once a run binds them.
const OPEN_BINDER: Vocabulary = Vocabulary {
    variables: true,
    groups: Groups::Outer,
};

/// The binary laws take the whole of whatever depth the tier asks for: the space two generated
/// types range over is the widest any law here draws from, and a thin sweep of it proves little.
///
/// A law may reject at most as many cases as it runs, so one whose draws meet its precondition
/// less than half the time fails with proptest's table of reasons rather than passing thinly.
fn binary() -> ProptestConfig {
    ProptestConfig {
        cases: crate::tests::case_share(1, 1),
        max_global_rejects: crate::tests::case_share(1, 1),
        ..ProptestConfig::default()
    }
}

/// [`binary`] for the laws whose draws meet their precondition almost always: a law may reject a
/// quarter as many cases as it runs, so a generator drifting off its precondition shows early.
fn strict() -> ProptestConfig {
    ProptestConfig {
        max_global_rejects: crate::tests::case_share(1, 4),
        ..binary()
    }
}

/// Three deep trees per case, so the ternary laws take three-eighths of the binary depth to land
/// near the same wall-clock — the ratio holds at whatever the tier sets, and so does the reject
/// budget's.
fn ternary() -> ProptestConfig {
    ProptestConfig {
        cases: crate::tests::case_share(3, 8),
        max_global_rejects: crate::tests::case_share(3, 8),
        ..ProptestConfig::default()
    }
}

// --- 1. Join and meet ---

proptest! {
    #![proptest_config(binary())]

    #[test]
    fn join_and_meet_are_commutative_and_idempotent(a in concrete(), b in concrete()) {
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        prop_assert_eq!(join(&types, scratch, a, b), join(&types, scratch, b, a));
        prop_assert_eq!(meet(&types, scratch, a, b), meet(&types, scratch, b, a));
        prop_assert_eq!(join(&types, scratch, a, a), a);
        prop_assert_eq!(meet(&types, scratch, a, a), a);
    }

    #[test]
    fn join_and_meet_absorb_each_other(a in concrete(), b in concrete()) {
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        prop_assert_eq!(join(&types, scratch, a, meet(&types, scratch, a, b)), a);
        prop_assert_eq!(meet(&types, scratch, a, join(&types, scratch, a, b)), a);
    }

    #[test]
    fn never_and_any_are_the_identities(a in concrete()) {
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
    fn join_and_meet_are_associative(a in small_concrete(), b in small_concrete(), c in small_concrete()) {
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
    fn the_order_is_reflexive_and_bounded(a in concrete()) {
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        prop_assert!(is_subtype_of(&types, scratch, a, a));
        prop_assert!(is_subtype_of(&types, scratch, KType::NEVER, a));
        prop_assert!(is_subtype_of(&types, scratch, a, KType::ANY));
    }

    #[test]
    fn the_order_is_antisymmetric((a, b) in arb_ordered_pair(world(), 3)) {
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        prop_assert!(is_subtype_of(&types, scratch, a, b), "the pair is ordered");
        if is_subtype_of(&types, scratch, b, a) {
            prop_assert_eq!(a, b);
        }
    }

    #[test]
    fn the_order_agrees_with_join_and_meet(a in concrete(), b in concrete()) {
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        let below = is_subtype_of(&types, scratch, a, b);
        prop_assert_eq!(below, join(&types, scratch, a, b) == b);
        prop_assert_eq!(below, meet(&types, scratch, a, b) == a);
    }

    /// The rigid rule belongs to *fits*: the order relates concrete types only.
    #[test]
    fn below_a_rigid_variable_fit_itself_and_what_fits_its_lower_end(
        a in one(),
        b in arb_lexical(world()),
        same in any::<bool>(),
    ) {
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        let b = DeclaredType::Type(b);
        let a = if same { b } else { a };
        let lower = types.node(b.raw()).rigid_lower().expect("a rigid variable has a lower end");
        prop_assert_eq!(
            fits(&types, scratch, a, b),
            a == b || fits(&types, scratch, a, lower)
        );
    }

    #[test]
    fn fits_is_reflexive(a in one()) {
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        prop_assert!(fits(&types, scratch, a, a));
    }

    #[test]
    fn fits_contains_the_order((a, b) in arb_ordered_pair(world(), 3)) {
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        prop_assert!(is_subtype_of(&types, scratch, a, b), "the pair is ordered");
        prop_assert!(fits(&types, scratch, a, b));
        prop_assert!(satisfied_by(&types, scratch, b, a));
    }

    #[test]
    fn no_type_lies_below_two_family_tops(a in concrete()) {
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
    fn the_order_is_transitive((a, b, c) in arb_chain(world(), 2)) {
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        prop_assert!(is_subtype_of(&types, scratch, a, b), "the chain's first step is ordered");
        prop_assert!(is_subtype_of(&types, scratch, b, c), "the chain's second step is ordered");
        prop_assert!(is_subtype_of(&types, scratch, a, c));
    }

    #[test]
    fn fits_is_transitive((a, b, c) in arb_fits_chain(world(), 2)) {
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        prop_assert!(fits(&types, scratch, a, b), "the chain's first step fits");
        prop_assert!(fits(&types, scratch, b, c), "the chain's second step fits");
        prop_assert!(fits(&types, scratch, a, c));
    }
}

proptest! {
    #![proptest_config(binary())]

    /// The chains [`fits_is_transitive`] never draws: an opaque view's signature, a signature over
    /// a head parameter bounded as its carrier's source was, and a signature over another bound, a
    /// ground slot, or a parameter bounded by the slot's type, each slot holding the variable at a
    /// covariant or a contravariant position. A hidden bound read anywhere but a head's fit, or read
    /// there at a contravariant position, breaks the chain.
    #[test]
    fn fits_is_transitive_through_a_bounded_head_parameter(
        (a, b, c) in arb_bounded_head_chain(world()),
    ) {
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        let fits = |a: KType, b: KType| sig_fits(&types, scratch, a.raw(), b.raw()).is_ok();
        prop_assert!(fits(a, b), "a view fits the signature its source fits");
        if fits(b, c) {
            prop_assert!(fits(a, c));
        }
    }

    /// The chains [`fits_is_transitive`] almost never draws: a binder, its instance at a least
    /// instance, and a type whose two positions take that instance apart.
    #[test]
    fn fits_is_transitive_through_an_instance((a, b, c) in arb_instance_chain(world())) {
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        prop_assert!(fits(&types, scratch, a, b), "the binder fits its instance");
        prop_assert!(fits(&types, scratch, b, c), "the instance fits the split");
        prop_assert!(fits(&types, scratch, a, c), "the binder fits the split");
    }
}

proptest! {
    #![proptest_config(strict())]

    /// An instance is taken of a scheme exactly where the scheme fits the type it is wanted at:
    /// the door answers *fits*' instantiation clause, and only adds which instance.
    #[test]
    fn an_instance_exists_where_its_scheme_fits(
        (scheme, wanted) in prop_oneof![
            arb_wanted_instance(world(), 3, BINDER, Vocabulary::CLOSED),
            arb_wanted_instance(world(), 3, BINDER, Vocabulary::CONCRETE),
        ]
    ) {
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        let result = instance_under(&types, scratch, scheme, wanted);
        prop_assert_eq!(
            matches!(result, Err(InstanceFailure::NoInstance)),
            !fits(&types, scratch, scheme, wanted)
        );
    }

    /// A least instance under a concrete type is concrete, and the instance it makes fits the type
    /// it was wanted at. *Fits*, not the order: a signature is ordered by its applications, so an
    /// instance returning a signature fits a wanted one that asks no member without lying under it.
    #[test]
    fn an_instance_fits_the_type_it_is_wanted_at(
        (scheme, wanted) in arb_own_instance(world(), 3, CLOSED_BINDER, Vocabulary::CONCRETE)
    ) {
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        let wanted = types.concrete(wanted).expect("a concrete scheme's ground instance is concrete");
        let solution = instance_under(&types, scratch, scheme, wanted).ok();
        prop_assume!(solution.is_some());
        let solution = solution.expect("assumed");
        let instance = types
            .concrete(instantiate_quantified(&types, scratch, scheme, &solution))
            .expect("a concrete scheme's instance under a concrete type is concrete");
        prop_assert!(fits(&types, scratch, instance, wanted));
    }

    /// An instance made under a wanted type holding lexical variables holds at every run: with each
    /// lexical variable bound as a run binds it, the instance at the solution so bound fits the
    /// wanted type so bound, each entry lies within its variable's bound, and binding commutes with
    /// instantiating — the run's instance is the load's, bound.
    #[test]
    fn an_instance_over_lexical_variables_holds_at_every_binding(
        (scheme, wanted) in arb_own_instance(world(), 3, OPEN_BINDER, Vocabulary::CLOSED),
        draw in draw(),
    ) {
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        let solution = instance_under(&types, scratch, scheme, wanted).ok();
        prop_assume!(solution.is_some());
        let solution = solution.expect("assumed");
        prop_assert!(
            !types.is_concrete(wanted.raw()),
            "an instance at lexical bindings names a lexical variable"
        );
        let bind = |kt: Handle| instance(&types, scratch, kt, &draw);
        let run_solution: Vec<Handle> = solution.iter().map(|each| bind(each.raw())).collect();
        let made = instantiate_quantified(&types, scratch, scheme, &solution);
        let (run_made, run_wanted) = (bind(made.raw()), bind(wanted.raw()));
        // A scheme binding no nested group holds no variable a run leaves free.
        prop_assert!(
            types.is_concrete(run_made)
                && types.is_concrete(run_wanted)
                && run_solution.iter().all(|each| types.is_concrete(*each)),
            "a run binds every lexical variable"
        );
        prop_assert!(order::fits(&types, scratch, run_made, run_wanted));
        for (entry, bound) in run_solution.iter().zip(quantifier_bounds(&types, scheme.raw())) {
            prop_assert!(order::fits(&types, scratch, *entry, bound.raw()));
        }
        prop_assert_eq!(
            bind(instantiate_quantified(&types, scratch, scheme, &run_solution).raw()),
            run_made
        );
    }
}

// --- 3. Unions ---

proptest! {
    #![proptest_config(binary())]

    #[test]
    fn union_of_is_canonical(members in prop::collection::vec(concrete(), 1..5)) {
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
                    .map(|(_, other)| other)
                    .collect();
                let rest = types.union_of(scratch, &others);
                prop_assert!(
                    !is_subtype_of(&types, scratch, member, rest),
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
        members in prop::collection::vec(concrete(), 0..4),
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
        let (a, b) = (a.raw(), b.raw());
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        prop_assert_eq!(types.list(a), types.list(a));
        prop_assert_eq!(types.dict(a, b), types.dict(a, b));
        // A node read back out and re-interned names the same handle.
        prop_assert_eq!(types.intern(scratch, types.node(a)), a);
        prop_assert_eq!(types.intern(scratch, types.node(b)), b);
    }

    /// The probe flags interning stores beside a node answer what a walk over the type would: a free
    /// quantifier reachable without crossing a shape's binder, any rigid variable reachable at all,
    /// anything parametric, and an opaque carrier.
    #[test]
    fn the_probe_flags_are_their_walks(declared in one()) {
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        let a = declared.raw();
        let quantified = visit(&types, scratch, a, &mut |_, node, _| match node {
            // Either binder's group is its own, so nothing under one is free here.
            _ if node.binds_quantifiers() => Visit::Skip,
            TypeNode::Quantified { .. } => Visit::Stop,
            _ => Visit::Descend,
        });
        let rigid = visit(&types, scratch, a, &mut |_, node, _| match node {
            TypeNode::Quantified { .. }
            | TypeNode::Lexical { .. }
            | TypeNode::Parameter { .. } => Visit::Stop,
            _ => Visit::Descend,
        });
        // A quantified binder is parametric itself, as is every variable; a signature and a sealed
        // member are leaves to the walk.
        let parametric = visit(&types, scratch, a, &mut |_, node, _| match node {
            _ if node.binds_quantifiers() => Visit::Stop,
            TypeNode::Quantified { .. }
            | TypeNode::Lexical { .. }
            | TypeNode::Parameter { .. } => Visit::Stop,
            _ => Visit::Descend,
        });
        // An opaque carrier is concrete, and no variable.
        let carrier = visit(&types, scratch, a, &mut |_, node, _| match node {
            TypeNode::Carrier { .. } => Visit::Stop,
            _ => Visit::Descend,
        });
        prop_assert_eq!(types.contains_quantified(a), quantified);
        prop_assert_eq!(types.contains_rigid(a), rigid);
        prop_assert_eq!(types.is_concrete(a), !parametric);
        prop_assert_eq!(types.contains_carrier(a), carrier);
        // The checked conversion agrees: a scheme is never concrete, and a type is where no
        // variable is reachable from it.
        let concrete = match declared {
            DeclaredType::Type(kt) => types.concrete(kt).is_some(),
            DeclaredType::Scheme(_) => false,
        };
        prop_assert_eq!(concrete, !parametric);
    }
}

// --- 5. Substitution ---

proptest! {
    #![proptest_config(binary())]

    /// A type reads a head parameter exactly where substituting it changes the type: a binding of
    /// a parameter it does not read leaves it whole, and one it reads moves it, read at a fresh
    /// carrier, which no other member absorbs.
    #[test]
    fn a_type_mentions_a_head_parameter_where_substituting_it_moves_it(
        (name, a) in arb_over_head_parameter(world(), 2),
        b in concrete(),
    ) {
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        let substituted =
            |to: KType| substitute_parameters(&types, scratch, a, Members::from_pairs(scratch, [(name, to)]));
        if types.mentions_parameter(scratch, world().declared(a), name) {
            let fresh = types.carrier(name, KType::ANY, crate::types::ContentKey(u128::MAX));
            prop_assert_ne!(substituted(fresh), a);
        } else {
            prop_assert_eq!(substituted(b), a);
        }
    }

    #[test]
    fn substitution_of_nothing_is_the_identity(a in one(), b in one()) {
        let (a, b) = (a.raw(), b.raw());
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        prop_assert_eq!(substitute_quantified::<Handle>(&types, scratch, a, &[]), a);
        if !types.contains_quantified(a) {
            prop_assert_eq!(substitute_quantified(&types, scratch, a, &[b, b, b]), a);
        }
        let none: Members<'_, TypeSymbol, Handle> = Members::EMPTY;
        prop_assert_eq!(substitute_parameters(&types, scratch, a, none), a);
    }

    // A type holding a binder is not drawn: relating two of them runs the unifier, whose
    // completeness is not this law's subject.
    #[test]
    fn bounding_above_lies_over_every_instance(a in arb_any_with(world(), 3, Vocabulary::CLOSED)) {
        let a = a.raw();
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        let above = bound_above(&types, scratch, a);
        prop_assert!(!types.contains_rigid(above));
        prop_assert!(order::is_subtype_of(&types, scratch, a, above));
        let lowest = [KType::NEVER; 8];
        let instance = substitute_quantified(&types, scratch, a, &lowest);
        prop_assert!(order::is_subtype_of(&types, scratch, instance, above));
    }

    /// A type read at the lower ends of its variables fits itself read at their upper ends, a
    /// nested binder included: the two readings of one slot the judge compares before any call.
    /// Over a binder *fits* is the unifier's instantiation clause, so this law draws one against
    /// itself where the one above draws none.
    #[test]
    fn a_type_read_below_its_variables_fits_itself_read_above(a in one()) {
        let a = a.raw();
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        let below = read_through(&types, scratch, a, Side::Below, &mut |_, node| {
            Some(Variable::of(node)?.interval().raw())
        });
        let above = bound_above(&types, scratch, a);
        prop_assert!(order::fits(&types, scratch, below, above));
    }
}

// --- 6. Shapes ---

proptest! {
    #![proptest_config(strict())]

    /// Re-interning a shape through its door with the group it already carries returns the very
    /// handle: the group order is idempotent.
    #[test]
    fn interning_a_shapes_own_content_is_a_fixed_point(a in shape()) {
        let a = a.raw();
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        if let TypeNode::ExpressionShape {
            quantifiers,
            bounds,
            elements,
            classes,
            ret,
        } = types.node(a)
        {
            let again = types.shape_group(scratch, quantifiers, bounds, &elements[..], classes, ret);
            prop_assert_eq!(again.0, a);
        }
    }

    /// Every occurrence of a shape's own variable carries the bound the shape stores for it.
    #[test]
    fn a_shape_stores_the_bounds_its_occurrences_carry(a in shape()) {
        let a = a.raw();
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        let stored = quantifier_bounds(&types, a);
        let mut carried: Vec<Option<KType>> = vec![None; stored.len()];
        visit(&types, scratch, a, &mut |_, node, context| match *node {
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
            prop_assert!(carried[index].is_none_or(|carried| carried == *bound));
        }
    }

    /// The function twin of [`interning_a_shapes_own_content_is_a_fixed_point`].
    #[test]
    fn interning_a_functions_own_content_is_a_fixed_point(a in function()) {
        let a = a.raw();
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        if let TypeNode::KFunction {
            quantifiers,
            bounds,
            params,
            ret,
        } = types.node(a)
        {
            let again = types.function_group(scratch, quantifiers, bounds, params.raw(), ret);
            prop_assert_eq!(again.0, a);
        }
    }

    /// The function twin of [`a_shape_stores_the_bounds_its_occurrences_carry`].
    #[test]
    fn a_function_stores_the_bounds_its_occurrences_carry(a in function()) {
        let a = a.raw();
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        let stored = quantifier_bounds(&types, a);
        let mut carried: Vec<Option<KType>> = vec![None; stored.len()];
        visit(&types, scratch, a, &mut |_, node, context| match *node {
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
            prop_assert!(carried[index].is_none_or(|carried| carried == *bound));
        }
    }

    #[test]
    fn specificity_flips_when_its_arguments_swap(a in shape(), b in shape()) {
        let (a, b) = (a.raw(), b.raw());
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
        if !matches!(types.node(a.raw()), TypeNode::ExpressionShape { .. }) {
            let (a, b) = (a.raw(), b.raw());
            prop_assert_eq!(shape_specificity(&types, scratch, a, b), Specificity::Incomparable);
            prop_assert_eq!(shape_specificity(&types, scratch, b, a), Specificity::Incomparable);
            prop_assert!(!admits_shape(&types, scratch, a, b));
            prop_assert!(!admits_shape(&types, scratch, b, a));
        }
    }

    /// Over monomorphic shapes the ranking is pointwise *fits* folded class by class: the first class
    /// whose slots fit the pair one way only decides, and two rankings of one key are unrelated.
    /// *Fits*, not the order: a slot ranks by what it admits, and a signature asking more members
    /// fits one asking fewer without lying under it.
    #[test]
    fn monomorphic_specificity_is_the_lexicographic_pointwise_fold(
        (a, b) in arb_shape_pair(world(), 3, Vocabulary::CONCRETE),
    ) {
        let (a, b) = (a.raw(), b.raw());
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        prop_assert!(
            Shape::of(&types, a).is_some_and(|shape| shape.quantifiers.is_empty())
                && Shape::of(&types, b).is_some_and(|shape| shape.quantifiers.is_empty())
                && keys_equal(a, b, &types),
            "the pair is two monomorphic shapes over one key"
        );
        let actual = shape_specificity(&types, scratch, a, b);
        let (ranking, other) = (shape_classes(a, &types), shape_classes(b, &types));
        if ranking != other {
            prop_assert_eq!(actual, Specificity::Incomparable);
            return Ok(());
        }
        let left: Vec<Handle> = shape_slots(a, &types).collect();
        let right: Vec<Handle> = shape_slots(b, &types).collect();
        let class = |index: usize| ranking.get(index).map_or(index, |c| usize::from(*c));
        let mut expected = Specificity::Equal;
        for current in 0..left.len() {
            let in_class = || (0..left.len()).filter(|index| class(*index) == current);
            let more = in_class().all(|i| order::fits(&types, scratch, left[i], right[i]));
            let less = in_class().all(|i| order::fits(&types, scratch, right[i], left[i]));
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
    fn a_shape_below_another_admits_what_it_admits((a, b) in arb_shape_below(world(), 3)) {
        let (a, b) = (a.raw(), b.raw());
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        prop_assert!(
            Shape::of(&types, a).is_some_and(|shape| shape.quantifiers.is_empty())
                && Shape::of(&types, b).is_some_and(|shape| shape.quantifiers.is_empty())
                && keys_equal(a, b, &types),
            "the pair is two monomorphic shapes over one key"
        );
        prop_assume!(order::fits(&types, scratch, a, b));
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
    shape: Handle,
    arguments: &[KType],
) -> bool {
    let mut collector = Collector::<Handle>::new(scratch, quantifier_bounds(types, shape));
    for (slot, argument) in shape_slots(shape, types).zip(arguments) {
        if admits(
            types,
            scratch,
            slot,
            argument.raw(),
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
    fn admission_without_quantifiers_is_the_order(
        a in arb_any_with(world(), 3, Vocabulary::CLOSED),
        b in one(),
    ) {
        let (a, b) = (a.raw(), b.raw());
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        let mut collector = Collector::<Handle>::new(scratch, &[]);
        let admitted = admits(&types, scratch, a, b, Variance::Co, &mut collector).is_ok();
        prop_assert_eq!(admitted, order::satisfied_by(&types, scratch, a, b));
    }

    /// A carrier is an atom: it lies under itself, a union holding it and `Any`, and above only
    /// itself and `Never`, in the order and in *fits* alike.
    #[test]
    fn a_carrier_lies_under_any_alone(
        a in concrete(),
        b in one(),
        carrier in arb_opaque(world()),
    ) {
        let (a, b, carrier) = (a.raw(), b.raw(), carrier.raw());
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        let holding = a == carrier
            || a == Handle::ANY
            || matches!(types.node(a), TypeNode::Union { members } if members.contains(&carrier));
        prop_assert_eq!(order::is_subtype_of(&types, scratch, carrier, a), holding);
        let atom = |x: Handle| x == carrier || x == Handle::NEVER;
        prop_assert_eq!(order::is_subtype_of(&types, scratch, a, carrier), atom(a));
        prop_assert_eq!(order::fits(&types, scratch, b, carrier), atom(b));
    }

    #[test]
    fn a_carried_variable_is_admitted_where_its_bound_is(a in one(), b in arb_lexical(world())) {
        let (a, b) = (a.raw(), b.raw());
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        let bound = types.node(b).rigid_bound().expect("a rigid variable has a bound");
        // A generated type holds no free quantifier, so a declared side that has one is built:
        // `a | LIST OF X` reaches the unifier's leaf through the list, and a union bound spanning
        // `a`'s members and the list reaches its member-by-member fallback.
        let variable = types.list(types.quantified(0, KType::ANY).raw());
        for declared in [a, types.union_of(scratch, &[a, variable])] {
            let mut through_bound = Collector::<Handle>::new(scratch, &[KType::ANY]);
            if admits(&types, scratch, declared, bound.raw(), Variance::Co, &mut through_bound)
                .is_err()
            {
                continue;
            }
            let mut collector = Collector::<Handle>::new(scratch, &[KType::ANY]);
            prop_assert!(
                admits(&types, scratch, declared, b, Variance::Co, &mut collector).is_ok(),
                "a position its bound fills refused the variable",
            );
        }
    }

    /// A type over a group's variables lies under `Any` and above `Never` whatever they solve to,
    /// so the unifier admits both extremes at every slot, a structural one included. Each variable
    /// binds as a bare one given the extreme part by part would: `Never` where it pairs
    /// covariantly, its bound where it pairs only contravariantly or not at all.
    #[test]
    fn a_solve_puts_every_type_under_any_and_over_never(
        (a, _) in arb_argument_pair(world(), 3, BINDER),
    ) {
        let a = a.raw();
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        let bounds = quantifier_bounds(&types, a);
        for slot in shape_slots(a, &types) {
            for (carried, variance) in [(Handle::NEVER, Variance::Co), (Handle::ANY, Variance::Contra)] {
                let mut collector = Collector::<Handle>::new(scratch, bounds);
                prop_assert!(
                    admits(&types, scratch, slot, carried, variance, &mut collector).is_ok(),
                    "a slot refused an extreme of the order",
                );
                let solution = collector.solve(&types);
                prop_assert!(solution.is_ok());
                let mut covariant = vec![false; bounds.len()];
                visit_free_quantified(&types, scratch, slot, variance, &mut |index, context| {
                    covariant[index] |= context.variance() == Variance::Co;
                    Visit::Descend
                });
                for (index, solved) in solution.expect("asserted").iter().enumerate() {
                    let expected = if covariant[index] { Handle::NEVER } else { bounds[index].raw() };
                    prop_assert_eq!(*solved, expected, "a variable bound unlike a bare one");
                }
            }
        }
    }

    #[test]
    fn a_solution_is_the_least_instance_of_its_contributions(
        (a, b) in arb_argument_pair(world(), 3, BINDER),
    ) {
        let (a, b) = (a.raw(), b.raw());
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        let bounds = quantifier_bounds(&types, a);
        prop_assert!(!bounds.is_empty(), "the candidate binds a group");
        let slots: Vec<Handle> = shape_slots(a, &types).collect();
        let arguments: Vec<Handle> = shape_slots(b, &types).collect();
        prop_assert!(keys_equal(a, b, &types), "the pair shares one key");
        let mut collector = Collector::<Handle>::new(scratch, bounds);
        prop_assume!(slots.iter().zip(arguments.iter()).all(|(slot, argument)| {
            admits(&types, scratch, *slot, *argument, Variance::Co, &mut collector).is_ok()
        }));
        let solution = match collector.solve(&types) {
            Ok(solution) => solution,
            // A solve fails only where the set its pair denotes is empty.
            Err(UnifyFailure::Disagree { lower, upper, .. }) => {
                prop_assert!(!order::fits(&types, scratch, lower, upper));
                return Ok(());
            }
            Err(UnifyFailure::Mismatch) => return Ok(()),
        };
        // Substituting the solution into the declared slots yields positions the arguments fill.
        for (slot, argument) in slots.iter().zip(arguments.iter()) {
            let solved = substitute_quantified(&types, scratch, *slot, &solution);
            prop_assert!(order::satisfied_by(&types, scratch, solved, *argument));
        }
        // Admission does not depend on the order the slots are read.
        let mut backwards = Collector::<Handle>::new(scratch, bounds);
        for (slot, argument) in slots.iter().zip(arguments.iter()).rev() {
            prop_assert!(
                admits(&types, scratch, *slot, *argument, Variance::Co, &mut backwards).is_ok()
            );
        }
        let again = backwards.solve(&types).ok();
        prop_assert_eq!(again.as_deref(), Some(&solution[..]));

        for (index, solved) in solution.iter().enumerate() {
            let (lower, upper) = collector.contributions(index);
            let bound = collector.bound(index).raw();
            // The least instance of the pair: its lower end where a lower contribution reached the
            // variable, and its upper end otherwise. An end is the extremum of its contributions
            // where they have one: the very handle over concrete contributions, and over
            // parametric ones a handle lying under it and over it, since a parametric operand
            // joins to the union, which keeps a variable beside a member the rigid rule orders
            // with it.
            let agrees = |solved: Handle, extremum: Handle, concrete: bool| {
                if concrete {
                    solved == extremum
                } else {
                    order::is_subtype_of(&types, scratch, solved, extremum)
                        && order::is_subtype_of(&types, scratch, extremum, solved)
                }
            };
            if !lower.is_empty() {
                prop_assert_eq!(*solved, lattice::join_iter(&types, scratch, lower.iter().copied()));
                for ceiling in upper.iter().chain([&bound]) {
                    prop_assert!(order::fits(&types, scratch, *solved, *ceiling));
                }
                let maximum = lower.iter().find(|candidate| {
                    lower.iter().all(|other| order::is_subtype_of(&types, scratch, *other, **candidate))
                });
                if let Some(maximum) = maximum {
                    let concrete = lower.iter().all(|each| types.is_concrete(*each));
                    prop_assert!(agrees(*solved, *maximum, concrete));
                }
            } else {
                let met = upper
                    .iter()
                    .fold(bound, |met, each| lattice::meet_through_variables(&types, scratch, met, *each));
                prop_assert_eq!(*solved, met);
                let minimum = upper.iter().find(|candidate| {
                    upper.iter().chain([&bound]).all(|other| {
                        order::is_subtype_of(&types, scratch, **candidate, *other)
                    })
                });
                if let Some(minimum) = minimum {
                    let concrete = upper.iter().all(|each| types.is_concrete(*each));
                    prop_assert!(agrees(*solved, *minimum, concrete));
                }
            }
            // Every solution fits its variable's declared bound.
            prop_assert!(order::fits(&types, scratch, *solved, bound));
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

/// A candidate shape over anything, and a shape over its key binding no group, mostly its own
/// instance at pool types or lexical variables over them, whose slots a call's static types may be
/// drawn from.
fn candidate_pair() -> BoxedStrategy<(DeclaredType<Parametric>, DeclaredType<Parametric>)> {
    arb_argument_pair(world(), 3, Vocabulary::ANY)
}

/// A [`Draw`], weighted toward the candidate's own slot and the argument shape's, mostly its
/// instance, so the static solve succeeds often enough for the laws to bite.
fn draw() -> impl Strategy<Value = Draw> {
    draw_from(prop_oneof![4 => Just(0u8), 2 => Just(1u8), 1 => Just(2u8)])
}

/// A [`Draw`] for the laws that need a call to be admitted: a static type from the pool is almost
/// never admitted, so only the candidate's own slots and the argument shape's are drawn from.
fn admitted_draw() -> impl Strategy<Value = Draw> {
    draw_from(prop_oneof![2 => Just(0u8), 1 => Just(1u8)])
}

/// A [`Draw`] whose static types come from where `source` says.
fn draw_from(source: impl Strategy<Value = u8>) -> impl Strategy<Value = Draw> {
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
fn instance(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    kt: Handle,
    draw: &Draw,
) -> Handle {
    read_through(
        types,
        scratch,
        kt,
        Side::Above,
        &mut |_, node| match Variable::of(node)? {
            Variable::Lexical {
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
                }
                .raw(),
            )),
            _ => None,
        },
    )
}

/// Static arguments for `a`, one per slot, beside carried arguments within them. Slot `k`'s static
/// type is drawn by `draw.picks[k]` from `a`'s own slot at its group's bounds, from one of `b`'s so
/// read, or from the argument pool. Its mode is the pick's second element: `1` is exactly the
/// static type, carried as the run carries it; `2`, where the static type holds no rigid variable
/// and meets a drawn type above `Never`, is bounded below by that meet, which it carries; anything
/// else is within the static type, carrying it met with a drawn type, or as the run carries it where
/// that meet is `Never`. `None` where some static type is `Never`.
fn static_and_carried(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    a: Handle,
    b: Handle,
    draw: &Draw,
) -> Option<(Vec<Interval<Handle>>, Vec<Handle>)> {
    // Each slot at its group's bounds: the instance a call reaches when every variable takes its
    // bound.
    let at_bounds = |shape: Handle| -> Vec<Handle> {
        let bounds = quantifier_bounds(types, shape);
        shape_slots(shape, types)
            .map(|slot| substitute_quantified(types, scratch, slot, bounds))
            .collect()
    };
    let (own, other) = (at_bounds(a), at_bounds(b));
    let mut arguments = Vec::with_capacity(own.len());
    let mut carried = Vec::with_capacity(own.len());
    for (k, (source, mode)) in draw.picks.iter().take(own.len()).enumerate() {
        let static_type = match source {
            0 => own[k],
            1 if !other.is_empty() => other[k % other.len()],
            _ => draw.pool[k].raw(),
        };
        let run = instance(types, scratch, static_type, draw);
        let below =
            lattice::meet_through_variables(types, scratch, static_type, draw.drawn[k].raw());
        let (argument, one) = match mode {
            1 => (Interval::point(static_type), run),
            2 if !types.contains_rigid(static_type) && below != Handle::NEVER => (
                Interval {
                    lower: below,
                    upper: static_type,
                },
                below,
            ),
            _ => (
                Interval {
                    lower: Handle::NEVER,
                    upper: static_type,
                },
                match lattice::meet_through_variables(types, scratch, run, draw.drawn[k].raw()) {
                    Handle::NEVER => run,
                    met => met,
                },
            ),
        };
        if argument.upper == Handle::NEVER || one == Handle::NEVER {
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
    interval: Interval<Handle>,
    draw: &Draw,
) -> Interval<Handle> {
    Interval {
        lower: instance(types, scratch, interval.lower, draw),
        upper: instance(types, scratch, interval.upper, draw),
    }
}

/// Whether `kt` lies within `interval`.
fn within(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    kt: Handle,
    interval: Interval<Handle>,
) -> bool {
    order::is_subtype_of(types, scratch, interval.lower, kt)
        && order::is_subtype_of(types, scratch, kt, interval.upper)
}

/// `shape`'s group solved jointly over one argument per slot, or `None`.
fn solve_jointly<'s>(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'s>,
    shape: Handle,
    arguments: &[Handle],
) -> Option<BumpVec<'s, Handle>> {
    collect_jointly(types, scratch, shape, arguments).0
}

/// [`solve_jointly`]'s solution beside the collector it ran in.
fn collect_jointly<'s>(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'s>,
    shape: Handle,
    arguments: &[Handle],
) -> (Option<BumpVec<'s, Handle>>, Collector<'s, Handle>) {
    let mut collector = Collector::<Handle>::new(scratch, quantifier_bounds(types, shape));
    for (slot, argument) in shape_slots(shape, types).zip(arguments) {
        if admits(
            types,
            scratch,
            slot,
            *argument,
            Variance::Co,
            &mut collector,
        )
        .is_err()
        {
            return (None, collector);
        }
    }
    (collector.solve(types).ok(), collector)
}

proptest! {
    #![proptest_config(binary())]

    /// For carried types each within its static interval, every solution over the carried types
    /// lies in the interval reported over the static ones.
    #[test]
    fn a_carried_solution_lies_in_its_static_interval(
        (a, b) in candidate_pair(),
        draw in admitted_draw(),
    ) {
        let (a, b) = (a.raw(), b.raw());
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        let pair = static_and_carried(&types, scratch, a, b, &draw);
        prop_assume!(pair.is_some());
        let (arguments, carried) = pair.expect("assumed");
        let run_interval = |interval| carried_interval(&types, scratch, interval, &draw);
        let uppers: Vec<Handle> = arguments.iter().map(|argument| argument.upper).collect();
        let (statics, collector) = collect_jointly(&types, scratch, a, &uppers);
        let solution = solve_jointly(&types, scratch, instance(&types, scratch, a, &draw), &carried);
        prop_assume!(statics.is_some() && solution.is_some());
        let (statics, solution) = (statics.expect("assumed"), solution.expect("assumed"));
        let slots: Vec<Handle> = shape_slots(a, &types).collect();
        // Over exact arguments the static solve is the call's own where a run reproduces it.
        let all_exact = arguments.iter().all(|argument| argument.is_exact())
            && collector.reproducible(&types);
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

    /// A reproducible solve commutes with binding: where the collector reports a static solve the
    /// same at every binding of the lexical variables it names, binding each as a run does and then
    /// solving gives the static solution so bound, and fails exactly where the static solve fails.
    #[test]
    fn a_reproducible_solve_commutes_with_binding(
        (a, b) in candidate_pair(),
        draw in draw(),
    ) {
        let (a, b) = (a.raw(), b.raw());
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        let pair = static_and_carried(&types, scratch, a, b, &draw);
        prop_assume!(pair.is_some());
        let (arguments, _) = pair.expect("assumed");
        let uppers: Vec<Handle> = arguments.iter().map(|argument| argument.upper).collect();
        // A closed solve is the closed laws' to state.
        prop_assume!(
            types.contains_rigid(a) || uppers.iter().any(|upper| types.contains_rigid(*upper))
        );
        let (statics, collector) = collect_jointly(&types, scratch, a, &uppers);
        prop_assume!(collector.reproducible(&types));
        let bound: Vec<Handle> = uppers
            .iter()
            .map(|upper| instance(&types, scratch, *upper, &draw))
            .collect();
        let run = solve_jointly(&types, scratch, instance(&types, scratch, a, &draw), &bound);
        prop_assert_eq!(run.is_some(), statics.is_some(), "a run failed where the load did not, or the other way");
        if let (Some(statics), Some(run)) = (statics, run) {
            for (solved, ran) in statics.iter().zip(run.iter()) {
                prop_assert_eq!(
                    instance(&types, scratch, *solved, &draw),
                    *ran,
                    "a run's solution is not the static one bound",
                );
            }
        }
    }

    /// A solve reads only its solving slots: with other calls' arguments at the slots that solve
    /// nothing, wherever the shape admits them, the solution is the same. A slot the door misses
    /// would carry another call's argument into the solve.
    #[test]
    fn a_solution_reads_only_its_solving_slots(
        (a, b) in candidate_pair(),
        draw in admitted_draw(),
        others in prop::collection::vec(draw(), 4),
    ) {
        let (a, b) = (a.raw(), b.raw());
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        let pair = static_and_carried(&types, scratch, a, b, &draw);
        prop_assume!(pair.is_some());
        let (_, carried) = pair.expect("assumed");
        let run_shape = instance(&types, scratch, a, &draw);
        let solution = admit_by_class(&types, scratch, run_shape, &carried);
        prop_assume!(solution.is_some());
        let solution = solution.expect("assumed");
        let solving = solving_slots(&types, scratch, run_shape);
        for other in &others {
            let Some((_, elsewhere)) = static_and_carried(&types, scratch, a, b, other) else {
                continue;
            };
            // The other call's arguments at every slot that solves nothing at once, then at each
            // alone: one argument that misses its slot leaves the others to be checked.
            let swapped = |at: &dyn Fn(usize) -> bool| -> Vec<Handle> {
                (0..carried.len())
                    .map(|slot| if at(slot) { elsewhere[slot] } else { carried[slot] })
                    .collect()
            };
            let mut mixes = vec![swapped(&|slot| !solving[slot])];
            mixes.extend(
                (0..carried.len())
                    .filter(|slot| !solving[*slot])
                    .map(|only| swapped(&|slot| slot == only)),
            );
            for mixed in mixes {
                if let Some(mixed) = admit_by_class(&types, scratch, run_shape, &mixed) {
                    prop_assert_eq!(mixed, solution, "a solve read a slot that solves nothing");
                }
            }
        }
    }
}

proptest! {
    #![proptest_config(strict())]

    /// A verdict holds of every call within the static types: *always* admits, *never* does not,
    /// and a solution lies in the intervals judging reported.
    #[test]
    fn a_verdict_holds_of_every_call_within_its_static_types(
        (a, b) in candidate_pair(),
        draw in draw(),
    ) {
        let (a, b) = (a.raw(), b.raw());
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        let pair = static_and_carried(&types, scratch, a, b, &draw);
        prop_assume!(pair.is_some());
        let (arguments, carried) = pair.expect("assumed");
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
                    within(&types, scratch, *solved, run_interval(interval.raw())),
                    "a class-by-class solution left its judged interval",
                );
            }
        }
    }
}

proptest! {
    #![proptest_config(strict())]

    /// A union none of whose binding members tie admits a carried type through one member whatever
    /// order its members are listed in: the order they are tried in turns on specificity, never on
    /// storage. The members are subterms of a drawn binder that name its own group, and lists of
    /// them, kept only where they tie with none kept before; each is tried against the drawn type
    /// and against every member's greatest instance.
    #[test]
    fn an_untied_union_admits_through_one_member_in_any_order(
        binder in prop_oneof![
            arb_own_instance(world(), 3, BINDER, Vocabulary::CONCRETE)
                .prop_map(|(scheme, _)| scheme.raw()),
            arb_shape_pair(world(), 3, BINDER).prop_map(|(shape, _)| shape.raw()),
        ],
        drawn in concrete(),
    ) {
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        // Every distinct subterm naming the binder's own group, its positions and what lies in them,
        // each beside a list of itself; up to three kept that tie with none kept before.
        let mut subterms: Vec<Handle> = Vec::new();
        visit(&types, scratch, binder, &mut |kt, node, context| {
            if context.binder_depth() > 1 || (node.binds_quantifiers() && context.binder_depth() > 0) {
                return Visit::Skip;
            }
            if context.binder_depth() == 1 && types.contains_quantified(kt) {
                subterms.push(kt);
            }
            Visit::Descend
        });
        let mut members: Vec<Handle> = Vec::new();
        for candidate in subterms.iter().flat_map(|kt| [*kt, types.list(*kt)]) {
            if members.len() < 3
                && !members.contains(&candidate)
                && !members.iter().any(|kept| ties(&types, scratch, *kept, candidate))
            {
                members.push(candidate);
            }
        }
        // Only a binder whose group no position names, or whose every pair ties, is left short.
        prop_assume!(members.len() >= 2);
        let orders: Vec<Vec<Handle>> = match members[..] {
            [a, b] => vec![vec![a, b], vec![b, a]],
            [a, b, c] => vec![
                vec![a, b, c],
                vec![a, c, b],
                vec![b, a, c],
                vec![b, c, a],
                vec![c, a, b],
                vec![c, b, a],
            ],
            _ => unreachable!("two or three members"),
        };
        // No value carries `Never`, which every member admits.
        let mut carried = vec![drawn.raw()];
        carried.extend(members.iter().map(|member| bound_above(&types, scratch, *member)));
        for one in carried.into_iter().filter(|one| *one != KType::NEVER.raw()) {
            let first = |order: &[Handle]| {
                most_determined_first(&types, scratch, order, one)
                    .iter()
                    .copied()
                    .find(|member| {
                        let mut collector = Collector::<Handle>::new(scratch, &[]);
                        admits(&types, scratch, *member, one, Variance::Co, &mut collector).is_ok()
                    })
            };
            let admitted = first(&orders[0]);
            for order in &orders[1..] {
                prop_assert_eq!(first(order), admitted);
            }
        }
    }
}

// --- 8. Sealing ---

proptest! {
    #![proptest_config(binary())]

    #[test]
    fn sealing_is_order_insensitive_and_idempotent(
        reprs in prop::collection::vec(small_concrete(), 1..4),
    ) {
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

    /// *Fits* holds of a signature type and itself and of anything and `Module`, and the meet of
    /// two signature types fits both.
    #[test]
    fn fits_bounds_the_signature_meet(a in signature_type(), b in signature_type()) {
        let (a, b) = (a.raw(), b.raw());
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        prop_assert!(
            is_signature_type(&types, a) && is_signature_type(&types, b),
            "the pair is two signature types"
        );
        prop_assert!(sig_fits(&types, scratch, a, a).is_ok());
        prop_assert!(sig_fits(&types, scratch, a, Handle::EMPTY_SIGNATURE).is_ok());
        let met = lattice::meet_through_variables(&types, scratch, a, b);
        prop_assert!(sig_fits(&types, scratch, met, a).is_ok());
        prop_assert!(sig_fits(&types, scratch, met, b).is_ok());
    }

    /// For two signature types the order is R-5 over their applications, and nothing solves.
    #[test]
    fn the_signature_order_is_its_rule(a in signature_type(), b in signature_type()) {
        let (a, b) = (a.raw(), b.raw());
        let types = registry();
        let bump = Bump::new();
        let scratch = &bump;
        let lower = applications(&types, scratch, a).expect("a signature type has applications");
        let upper = applications(&types, scratch, b).expect("a signature type has applications");
        prop_assert_eq!(
            order::is_subtype_of(&types, scratch, a, b),
            applications_under(&lower, &upper)
        );
    }

    /// Every interned schema is stored in its canonical order — the invariant that lets every
    /// reader walk a table straight through and look a name up by binary search.
    #[test]
    fn a_schema_is_stored_in_canonical_order(a in arb_signature(world(), 3)) {
        let types = registry();
        let TypeNode::Signature { schema, .. } = types.node(a) else {
            unreachable!("a drawn signature is one");
        };
        prop_assert!(schema.parameters.windows(2).all(|w| w[0].0 < w[1].0));
        prop_assert!(schema.manifest_members.windows(2).all(|w| w[0].0 < w[1].0));
        prop_assert!(schema.value_slots.windows(2).all(|w| w[0].0 < w[1].0));
        prop_assert!(
            schema
                .operators
                .iter()
                .all(|group| group.members.windows(2).all(|w| w[0] < w[1]))
        );
    }
}

// --- 10. Rendering ---

proptest! {
    #![proptest_config(binary())]

    #[test]
    fn rendering_is_total_and_deterministic(a in one()) {
        let world = world();
        let once = crate::types::display_name(a, &world.types, &world.symbols).to_string();
        let twice = crate::types::display_name(a, &world.types, &world.symbols).to_string();
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
                    index == peer || !order::is_subtype_of(&types, scratch, other.raw(), one.raw()),
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
                        .any(|survivor| {
                            order::is_subtype_of(&types, scratch, survivor.raw(), dropped.raw())
                        }),
                "an overload was dropped with no survivor below it",
            );
        }
    }
}
