//! The relations between two signature schemas, and the specificity verdict a dispatch ranks
//! candidates by.
//!
//! [`sig_subtype`] is the canonical relation: `sub <: sup` iff `sub` supplies every member `sup`
//! names, with each manifest member equal, each abstract member present at the matching kind and
//! under the declared bound, each value slot covariantly compatible after abstract-member
//! substitution, each keyworded member satisfied by an overload the same selection dispatch would
//! make, and each operator record covered at an equal mode.
//!
//! [`meet_schemas`] is what two signatures meet at in [`meet`](super::lattice::meet): the module
//! lattice has no join of its own, since two unordered signatures join to their union.

use std::collections::HashMap;

use crate::memory::ScopeId;
use crate::parse::{IdentityBuildHasher, KeywordSymbol, TypeSymbol, ValueSymbol};

use super::handle::KType;
use super::lattice::meet;
use super::node::TypeNode;
use super::operators::ReductionMode;
use super::order::{dominant, is_more_specific_than, is_subtype_of, satisfied_by};
use super::registry::TypeRegistry;
use super::schema::{
    DeclaredGroup, OperatorMembers, SigSchema, TypeMemberMap, canonical_groups,
    canonical_overloads, constructor_param_names, elements_key_equal, is_shape, merged_bindings,
    name_sets_equal, shape_keys_equal, shape_quantifiers, shape_slots,
};
use super::shape::{DispatchTokenElement, Specificity};
use super::substitute::{slot_satisfied_by, substitute_sig_members};
use super::unify::{Collector, admits_with};
use super::walk::Variance;

// --- Specificity ---

/// Whether [`admits_shape`] compares the return positions. Dispatch never selects on a return, so
/// specificity leaves them out; the order's instantiation clause reads them.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Returns {
    Ignored,
    Checked,
}

/// Whether `declared` admits `candidate` position by position — `declared`'s variables solved,
/// `candidate`'s rigid — and, under [`Returns::Checked`], `declared`'s return under `candidate`'s.
///
/// The one door specificity, keyworded selection, interface canonicalization and the order's
/// shape-instantiation clause all rank through, so none can drift. Prenex instantiation through
/// the collector: each slot pair asks the candidate's slot to lie under the declared one
/// (covariant for the collector, since a slot's own polarity is contravariant), the return pair
/// asks the declared return to lie under the candidate's, and `solve` decides. The candidate's
/// `Quantified` nodes fall to the rigid rule automatically, because the collector only ever solves
/// declared-side variables and the carried side is never substituted. Two things that are not both
/// shapes under one key admit nothing.
pub(super) fn admits_shape(
    types: &TypeRegistry,
    declared: KType,
    candidate: KType,
    returns: Returns,
) -> bool {
    types.with_node(declared, |declared_node| {
        types.with_node(candidate, |candidate_node| {
            let (
                TypeNode::ExpressionShape {
                    quantifiers,
                    elements: declared_elements,
                    ret: declared_ret,
                },
                TypeNode::ExpressionShape {
                    elements: candidate_elements,
                    ret: candidate_ret,
                    ..
                },
            ) = (declared_node, candidate_node)
            else {
                return false;
            };
            if !elements_key_equal(declared_elements, candidate_elements) {
                return false;
            }
            let mut collector = Collector::new(quantifiers.len());
            for pair in declared_elements.iter().zip(candidate_elements.iter()) {
                if let (DispatchTokenElement::Slot(slot), DispatchTokenElement::Slot(argument)) =
                    pair
                    && admits_with(types, *slot, *argument, Variance::Co, &mut collector).is_err()
                {
                    return false;
                }
            }
            if returns == Returns::Checked
                && admits_with(
                    types,
                    *declared_ret,
                    *candidate_ret,
                    Variance::Contra,
                    &mut collector,
                )
                .is_err()
            {
                return false;
            }
            collector.solve(types).is_ok()
        })
    })
}

/// Rank two candidates under one bucket key by mutual admission.
///
/// `a` is at least as specific as `b` when `b` admits `a`'s slot types as arguments. For
/// monomorphic shapes this is the pointwise fold of the order over paired slots; for a generic
/// candidate it is the classic "more specific method" rule, so `(f _ :Number)` beats
/// `(f FOR ALL (Elt) _ :Elt)` and `(f _ :Any)` ties with it.
pub fn shape_specificity(types: &TypeRegistry, a: KType, b: KType) -> Specificity {
    if !is_shape(a, types) || !is_shape(b, types) {
        // Two things that are not both shapes have no bucket in common to rank under, which is a
        // refusal rather than a tie: the empty element run a non-shape reads as would otherwise
        // make every pair of leaves compare `Equal`.
        return Specificity::Incomparable;
    }
    let more = admits_shape(types, b, a, Returns::Ignored);
    let less = admits_shape(types, a, b, Returns::Ignored);
    match (more, less) {
        (true, false) => Specificity::StrictlyMore,
        (false, true) => Specificity::StrictlyLess,
        (true, true) => Specificity::Equal,
        (false, false) => Specificity::Incomparable,
    }
}

/// The one-slot case of a specificity tournament, over the slot types alone: `Some(i)` iff
/// `candidates[i]` is strictly below every peer in the order.
pub fn most_specific_ktype(types: &TypeRegistry, candidates: &[KType]) -> Option<usize> {
    dominant(candidates.len(), |i, j| {
        is_more_specific_than(types, candidates[i], candidates[j])
    })
}

// --- The relation ---

/// Why a [`sig_subtype`] check failed — the per-member rule that rejected, carrying the offending
/// member's symbol and the handles that disagreed.
///
/// Symbols and handles rather than rendered text: rendering needs the label interner, which is the
/// caller's, so [`render_sig_failure`](super::render::render_sig_failure) produces the fragment.
#[derive(Clone, Debug)]
pub enum SigSubtypeFailure {
    MissingTypeMember {
        name: TypeSymbol,
    },
    ManifestMismatch {
        name: TypeSymbol,
        got: KType,
        expected: KType,
    },
    /// A type member's kind or parameter names disagreed. `expected_params` is `Some(names)` when
    /// the super signature declares a constructor over those parameters, `None` when it declares a
    /// first-order proper type.
    KindMismatch {
        name: TypeSymbol,
        expected_params: Option<Vec<TypeSymbol>>,
        got: KType,
    },
    /// The member is present at the right kind but does not lie under the declared bound.
    BoundMismatch {
        name: TypeSymbol,
        got: KType,
        expected: KType,
    },
    MissingValueSlot {
        name: ValueSymbol,
    },
    ValueSlotMismatch {
        name: ValueSymbol,
        got: KType,
        expected: KType,
    },
    /// The sub schema declares no dispatch bucket under the declared member's key at all.
    MissingKeyworded {
        head: KType,
    },
    /// The bucket exists but no overload in it satisfies the declared member.
    KeywordedMismatch {
        head: KType,
        got: Vec<KType>,
    },
    /// Every overload under the key failed, and the first failed at a position the declared member
    /// **quantifies** over: a concrete position there says the module implements one instantiation
    /// where the signature declares an operation holding at every one.
    QuantifiedMismatch {
        head: KType,
        parameter: TypeSymbol,
        got: KType,
    },
    /// Two or more overloads satisfy the declared member and none is strictly the most specific —
    /// the keyworded reading of a dispatch ambiguity, raised where dispatch would raise it.
    AmbiguousKeyworded {
        head: KType,
        candidates: Vec<KType>,
    },
    /// No record in the module's operator registry covers the declared record's members.
    MissingOperatorGroup {
        members: Vec<KeywordSymbol>,
    },
    /// A record covering the declared members exists, but chains them a different way.
    OperatorModeMismatch {
        members: Vec<KeywordSymbol>,
        expected: ReductionMode,
        got: ReductionMode,
    },
}

/// `sub <: sup`. The failure is boxed: it is large relative to the common `Ok` path.
pub fn sig_subtype(
    types: &TypeRegistry,
    sub: &SigSchema,
    sup: &SigSchema,
) -> Result<(), Box<SigSubtypeFailure>> {
    // Every member type `sup` declares is read through `sub`'s bindings for `sup`'s own abstract
    // members, so a bound, a manifest binding and a value slot that name one of them all mean what
    // `sub` supplies. Substitution is the identity when `sup` declares nothing abstract.
    let bindings = sub.member_bindings();
    let read =
        |declared: KType| substitute_sig_members(types, declared, ScopeId::SENTINEL, &bindings);

    // 1. Abstract members: present at the matching kind, over the same parameter-name *set*, and
    // under the declared bound. Parameter names are interface: a family declaring `{Item}` does not
    // supply a slot declared over `{Elem}`.
    for (name, sup_repr) in &sup.abstract_members {
        let Some(sub_binding) = sub.type_member(*name) else {
            return Err(Box::new(SigSubtypeFailure::MissingTypeMember {
                name: *name,
            }));
        };
        let sup_params = constructor_param_names(*sup_repr, types);
        let sub_params = constructor_param_names(sub_binding, types);
        let agrees = match (&sup_params, &sub_params) {
            (None, None) => true,
            (Some(expected), Some(got)) => name_sets_equal(expected, got),
            _ => false,
        };
        if !agrees {
            return Err(Box::new(SigSubtypeFailure::KindMismatch {
                name: *name,
                expected_params: sup_params,
                got: sub_binding,
            }));
        }
        let expected = read(requirement(types, *sup_repr));
        let got = read(requirement(types, sub_binding));
        if !is_subtype_of(types, got, expected) {
            return Err(Box::new(SigSubtypeFailure::BoundMismatch {
                name: *name,
                got,
                expected,
            }));
        }
    }

    // 2. Manifest members: present manifest in `sub` with an equal type, once the requirement is
    // read through `sub`'s bindings — a manifest member may be declared *as* another member
    // (`Wrap = Elt`), and what that fixes it to is whatever `sub` binds `Elt` to.
    for (name, fixed) in &sup.manifest_members {
        let expected = read(*fixed);
        match sub.manifest_members.get(name).map(|got| read(*got)) {
            Some(got) if got == expected => {}
            Some(got) => {
                return Err(Box::new(SigSubtypeFailure::ManifestMismatch {
                    name: *name,
                    got,
                    expected,
                }));
            }
            // An abstract `sub` member supplies no witness for a manifest requirement.
            None => match sub.abstract_members.get(name) {
                Some(repr) => {
                    return Err(Box::new(SigSubtypeFailure::ManifestMismatch {
                        name: *name,
                        got: *repr,
                        expected,
                    }));
                }
                None => {
                    return Err(Box::new(SigSubtypeFailure::MissingTypeMember {
                        name: *name,
                    }));
                }
            },
        }
    }

    // 3. Value slots: present and covariantly compatible after abstract-member substitution.
    for (name, declared) in &sup.value_slots {
        let Some(carried) = sub.value_slots.get(name) else {
            return Err(Box::new(SigSubtypeFailure::MissingValueSlot {
                name: *name,
            }));
        };
        let carried = read(*carried);
        if !satisfied_by(types, read(*declared), carried) {
            return Err(Box::new(SigSubtypeFailure::ValueSlotMismatch {
                name: *name,
                got: carried,
                expected: *declared,
            }));
        }
    }

    // 4. Keyworded members, mirroring dispatch resolution: each declared overload needs at least one
    // satisfying overload in `sub`'s bucket under the same key, and among the satisfiers the most
    // specific is the one it selects. An incomparable tie is a dispatch ambiguity, and rejects here
    // rather than at the call.
    for declared in &sup.keyworded {
        let candidates: Vec<KType> = sub
            .keyworded
            .iter()
            .filter(|candidate| shape_keys_equal(*declared, **candidate, types))
            .map(|candidate| read(*candidate))
            .collect();
        // Both sides are read in `sub`'s world, so the selection runs the plain structural rule.
        match select_keyworded_satisfier(types, read(*declared), &candidates, None) {
            Ok(_) => {}
            Err(satisfiers) if satisfiers.is_empty() && candidates.is_empty() => {
                return Err(Box::new(SigSubtypeFailure::MissingKeyworded {
                    head: *declared,
                }));
            }
            Err(satisfiers) if satisfiers.is_empty() => {
                if let Some((parameter, got)) =
                    quantified_position_failure(types, *declared, candidates[0])
                {
                    return Err(Box::new(SigSubtypeFailure::QuantifiedMismatch {
                        head: *declared,
                        parameter,
                        got,
                    }));
                }
                return Err(Box::new(SigSubtypeFailure::KeywordedMismatch {
                    head: *declared,
                    got: candidates,
                }));
            }
            Err(satisfiers) => {
                return Err(Box::new(SigSubtypeFailure::AmbiguousKeyworded {
                    head: *declared,
                    candidates: satisfiers.iter().map(|i| candidates[*i]).collect(),
                }));
            }
        }
    }

    // 5. Operator members: each declared record needs a `sub` record whose member set **includes**
    // it — width, as for every other channel — under an **equal** mode. At most one `sub` record can
    // cover a declared one: two records in a channel never share a member. Mode is matched exactly
    // because a mode is not an approximation of another — a run folded right and the same run folded
    // left compute different things.
    for declared in &sup.operators {
        let covering = sub
            .operators
            .iter()
            .find(|record| declared.members.iter().all(|m| record.members.contains(m)));
        let Some(covering) = covering else {
            return Err(Box::new(SigSubtypeFailure::MissingOperatorGroup {
                members: declared.members.clone(),
            }));
        };
        if covering.mode != declared.mode {
            return Err(Box::new(SigSubtypeFailure::OperatorModeMismatch {
                members: declared.members.clone(),
                expected: declared.mode,
                got: covering.mode,
            }));
        }
    }
    Ok(())
}

/// What a type member requires of whatever binds it: a rigid variable's bound, and a manifest
/// binding's own type. The one reading the bound check on both sides of [`sig_subtype`] takes.
fn requirement(types: &TypeRegistry, member: KType) -> KType {
    types.with_node(member, |node| match node {
        TypeNode::AbstractType { bound, .. } => *bound,
        _ => member,
    })
}

/// Why `candidate` failed the declared member, when the reason is a **quantified** position: the
/// first slot whose declared type reads a quantifier and whose candidate slot is not at least as
/// general, as the parameter's name and the type the candidate fixes it to.
fn quantified_position_failure(
    types: &TypeRegistry,
    declared: KType,
    candidate: KType,
) -> Option<(TypeSymbol, KType)> {
    let quantifiers = shape_quantifiers(declared, types);
    if quantifiers.is_empty() {
        return None;
    }
    let candidate_slots = shape_slots(candidate, types);
    for (position, declared_slot) in shape_slots(declared, types).iter().enumerate() {
        let candidate_slot = candidate_slots.get(position)?;
        // Contravariance: a candidate position fills a declared one by being equal or more general.
        if is_subtype_of(types, *declared_slot, *candidate_slot) {
            continue;
        }
        let named =
            (0..quantifiers.len()).find(|i| types.references_quantifier(*declared_slot, *i))?;
        return Some((quantifiers[named], *candidate_slot));
    }
    None
}

/// The overload a declared keyworded member selects out of `candidates` — the one resolution both
/// [`sig_subtype`] and an ascription's bucket replay run, so the member a signature check accepts is
/// the member the view installs.
///
/// Two steps, mirroring dispatch: keep the candidates that **satisfy** the declared overload, then
/// rank the survivors by [`shape_specificity`] and take the one strictly more specific than every
/// peer. A lone satisfier wins with no ranking.
///
/// `substitution` is how a declared type that references a binder's abstract members is read. A
/// caller whose `declared` is already substituted into the candidates' own world passes `None` and
/// gets the plain structural rule.
///
/// `Err` carries the satisfier indices: empty means nothing satisfied, two or more an incomparable
/// tie.
pub fn select_keyworded_satisfier(
    types: &TypeRegistry,
    declared: KType,
    candidates: &[KType],
    substitution: Option<(&TypeMemberMap, ScopeId)>,
) -> Result<usize, Vec<usize>> {
    let satisfiers: Vec<usize> = candidates
        .iter()
        .enumerate()
        .filter(|(_, candidate)| match substitution {
            Some((members, id)) => slot_satisfied_by(types, declared, **candidate, id, members),
            None => satisfied_by(types, declared, **candidate),
        })
        .map(|(index, _)| index)
        .collect();
    if let [only] = satisfiers[..] {
        return Ok(only);
    }
    dominant(satisfiers.len(), |i, j| {
        matches!(
            shape_specificity(types, candidates[satisfiers[i]], candidates[satisfiers[j]]),
            Specificity::StrictlyMore
        )
    })
    .map(|i| satisfiers[i])
    .ok_or(satisfiers)
}

// --- Meet of schemas ---

/// The greatest lower bound of two schemas, or `None` when they make conflicting claims: two
/// manifest bindings for one name, two parameter-name sets for one member, or two chaining modes
/// for one operator run.
///
/// Width unions — a lower bound may promise everything either operand promises — and depth
/// reconciles per member, so a shared value slot meets and a shared type member takes the stronger
/// binding.
pub(super) fn meet_schemas(
    types: &TypeRegistry,
    a: &SigSchema,
    b: &SigSchema,
) -> Option<SigSchema> {
    let mut abstract_members = TypeMemberMap::default();
    let mut manifest_members = TypeMemberMap::default();
    let names: Vec<TypeSymbol> = a
        .abstract_members
        .keys()
        .chain(a.manifest_members.keys())
        .chain(b.abstract_members.keys())
        .chain(b.manifest_members.keys())
        .copied()
        .collect();
    for name in names {
        if abstract_members.contains_key(&name) || manifest_members.contains_key(&name) {
            continue;
        }
        match (a.type_member(name), b.type_member(name)) {
            (Some(left), Some(right)) => {
                let params = match (
                    constructor_param_names(left, types),
                    constructor_param_names(right, types),
                ) {
                    (None, None) => Vec::new(),
                    (Some(x), Some(y)) if name_sets_equal(&x, &y) => x,
                    _ => return None,
                };
                let left_manifest = a.manifest_members.contains_key(&name);
                let right_manifest = b.manifest_members.contains_key(&name);
                match (left_manifest, right_manifest) {
                    // A manifest binding is the stronger claim, so it survives — provided it lies
                    // under what the other side requires.
                    (true, true) if left != right => return None,
                    (true, true) => {
                        manifest_members.insert(name, left);
                    }
                    (true, false) => {
                        if !is_subtype_of(types, left, requirement(types, right)) {
                            return None;
                        }
                        manifest_members.insert(name, left);
                    }
                    (false, true) => {
                        if !is_subtype_of(types, right, requirement(types, left)) {
                            return None;
                        }
                        manifest_members.insert(name, right);
                    }
                    (false, false) => {
                        let bound =
                            meet(types, requirement(types, left), requirement(types, right));
                        abstract_members.insert(
                            name,
                            types.abstract_type(ScopeId::SENTINEL, name, params, None, bound),
                        );
                    }
                }
            }
            (Some(only), None) | (None, Some(only)) => {
                if a.abstract_members.contains_key(&name) || b.abstract_members.contains_key(&name)
                {
                    abstract_members.insert(name, only);
                } else {
                    manifest_members.insert(name, only);
                }
            }
            (None, None) => {}
        }
    }

    // A shared member takes a binding neither operand's own handle names — the bound is the meet of
    // theirs — so every type carried over from an operand reads its member references through the
    // result's bindings. References are by name, which is what makes that a rename rather than a
    // reinterpretation.
    let chosen = merged_bindings(&abstract_members, &manifest_members);
    for kt in manifest_members.values_mut() {
        *kt = substitute_sig_members(types, *kt, ScopeId::SENTINEL, &chosen);
    }
    let bindings = merged_bindings(&abstract_members, &manifest_members);
    let resolve = |kt: KType| substitute_sig_members(types, kt, ScopeId::SENTINEL, &bindings);

    let mut value_slots: HashMap<ValueSymbol, KType, IdentityBuildHasher> = HashMap::default();
    for (name, left) in &a.value_slots {
        value_slots.insert(*name, resolve(*left));
    }
    for (name, right) in &b.value_slots {
        let right = resolve(*right);
        let met = match value_slots.get(name) {
            Some(left) => meet(types, *left, right),
            None => right,
        };
        value_slots.insert(*name, met);
    }

    // A lower bound declares every overload either operand does; canonicalization then drops
    // whichever of a pair the other already admits.
    let keyworded = canonical_overloads(
        a.keyworded
            .iter()
            .chain(b.keyworded.iter())
            .map(|shape| resolve(*shape))
            .collect(),
        types,
    );

    let operators = merge_operator_records(a, b)?;

    Some(SigSchema {
        sig_id: (!abstract_members.is_empty()).then_some(ScopeId::SENTINEL),
        abstract_members,
        manifest_members,
        value_slots,
        keyworded,
        operators,
    })
}

/// Both operands' chaining records, merged so no two records in the result share a member — the
/// invariant a channel carries. Two records sharing a member must agree on the mode; disagreeing is
/// the absence of a meet.
fn merge_operator_records(a: &SigSchema, b: &SigSchema) -> Option<OperatorMembers> {
    let mut merged: OperatorMembers = Vec::new();
    for record in a.operators.iter().chain(b.operators.iter()) {
        let overlapping: Vec<usize> = merged
            .iter()
            .enumerate()
            .filter(|(_, held)| held.members.iter().any(|m| record.members.contains(m)))
            .map(|(index, _)| index)
            .collect();
        if overlapping.iter().any(|i| merged[*i].mode != record.mode) {
            return None;
        }
        let mut members = record.members.clone();
        for index in overlapping.iter().rev() {
            members.extend(merged.remove(*index).members);
        }
        members.sort_unstable();
        members.dedup();
        merged.push(DeclaredGroup {
            members,
            mode: record.mode,
        });
    }
    Some(canonical_groups(merged))
}
