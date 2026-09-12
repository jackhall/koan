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

use crate::memory::{BumpAllocator, BumpVec, ScopeId};
use crate::parse::{KeywordSymbol, TypeSymbol, ValueSymbol};

use super::handle::KType;
use super::lattice::meet;
use super::node::TypeNode;
use super::operators::ReductionMode;
use super::order::{dominant, is_more_specific_than, is_subtype_of, satisfied_by};
use super::registry::TypeRegistry;
use super::schema::{
    DeclaredGroup, Members, SchemaDraft, SigSchema, canonical_groups, constructor_param_names,
    elements_key_equal, is_shape, member, merge_join, merged_bindings, name_sets_equal,
    shape_keys_equal, shape_quantifiers, shape_slots,
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
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    declared: KType,
    candidate: KType,
    returns: Returns,
) -> bool {
    let (
        TypeNode::ExpressionShape {
            quantifiers,
            elements: declared_elements,
            ret: declared_ret,
            ..
        },
        TypeNode::ExpressionShape {
            elements: candidate_elements,
            ret: candidate_ret,
            ..
        },
    ) = (types.node(declared), types.node(candidate))
    else {
        return false;
    };
    if !elements_key_equal(declared_elements, candidate_elements) {
        return false;
    }
    let mut collector = Collector::new(scratch, quantifiers.len());
    for pair in declared_elements.iter().zip(candidate_elements) {
        if let (DispatchTokenElement::Slot(slot), DispatchTokenElement::Slot(argument)) = pair
            && admits_with(
                types,
                scratch,
                *slot,
                *argument,
                Variance::Co,
                &mut collector,
            )
            .is_err()
        {
            return false;
        }
    }
    if returns == Returns::Checked
        && admits_with(
            types,
            scratch,
            declared_ret,
            candidate_ret,
            Variance::Contra,
            &mut collector,
        )
        .is_err()
    {
        return false;
    }
    collector.solve(types).is_ok()
}

/// Rank two candidates under one bucket key by mutual admission.
///
/// `a` is at least as specific as `b` when `b` admits `a`'s slot types as arguments. For
/// monomorphic shapes this is the pointwise fold of the order over paired slots; for a generic
/// candidate it is the classic "more specific method" rule, so `(f _ :Number)` beats
/// `(f FOR ALL (Elt) _ :Elt)` and `(f _ :Any)` ties with it.
pub fn shape_specificity(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    a: KType,
    b: KType,
) -> Specificity {
    if !is_shape(a, types) || !is_shape(b, types) {
        // Two things that are not both shapes have no bucket in common to rank under, which is a
        // refusal rather than a tie: the empty element run a non-shape reads as would otherwise
        // make every pair of leaves compare `Equal`.
        return Specificity::Incomparable;
    }
    let more = admits_shape(types, scratch, b, a, Returns::Ignored);
    let less = admits_shape(types, scratch, a, b, Returns::Ignored);
    match (more, less) {
        (true, false) => Specificity::StrictlyMore,
        (false, true) => Specificity::StrictlyLess,
        (true, true) => Specificity::Equal,
        (false, false) => Specificity::Incomparable,
    }
}

/// The one-slot case of a specificity tournament, over the slot types alone: `Some(i)` iff
/// `candidates[i]` is strictly below every peer in the order.
pub fn most_specific_ktype(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    candidates: &[KType],
) -> Option<usize> {
    dominant(candidates.len(), |i, j| {
        is_more_specific_than(types, scratch, candidates[i], candidates[j])
    })
}

// --- The relation ---

/// Why a [`sig_subtype`] check failed — the per-member rule that rejected, carrying the offending
/// member's symbol and the handles that disagreed.
///
/// Symbols and handles rather than rendered text: rendering needs the label interner, which is the
/// caller's, so [`render_sig_failure`](super::render::render_sig_failure) produces the fragment.
/// `Copy` and unboxed: the relation's own negative verdicts build one, so a failure that allocated
/// would put an allocation on the order's path. A run it names is either the super schema's own
/// (`'run`) or staged in the caller's scratch (`'s`).
#[derive(Clone, Copy, Debug)]
pub enum SigSubtypeFailure<'run, 's> {
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
        expected_params: Option<&'run [TypeSymbol]>,
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
        got: &'s [KType],
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
        candidates: &'s [KType],
    },
    /// No record in the module's operator registry covers the declared record's members.
    MissingOperatorGroup {
        members: &'run [KeywordSymbol],
    },
    /// A record covering the declared members exists, but chains them a different way.
    OperatorModeMismatch {
        members: &'run [KeywordSymbol],
        expected: ReductionMode,
        got: ReductionMode,
    },
}

/// `sub <: sup`.
pub fn sig_subtype<'run, 's>(
    types: &TypeRegistry<'run>,
    scratch: BumpAllocator<'s>,
    sub: SigSchema<'run>,
    sup: SigSchema<'run>,
) -> Result<(), SigSubtypeFailure<'run, 's>> {
    // Every member type `sup` declares is read through `sub`'s bindings for `sup`'s own abstract
    // members, so a bound, a manifest binding and a value slot that name one of them all mean what
    // `sub` supplies. Substitution is the identity when `sup` declares nothing abstract.
    let bindings = sub.member_bindings(scratch);
    let read = |declared: KType| {
        substitute_sig_members(types, scratch, declared, ScopeId::SENTINEL, bindings)
    };

    // 1. Abstract members: present at the matching kind, over the same parameter-name *set*, and
    // under the declared bound. Parameter names are interface: a family declaring `{Item}` does not
    // supply a slot declared over `{Elem}`.
    for (name, sup_repr) in sup.abstract_members.iter().copied() {
        let Some(sub_binding) = sub.type_member(name) else {
            return Err(SigSubtypeFailure::MissingTypeMember { name });
        };
        let sup_params = constructor_param_names(sup_repr, types);
        let sub_params = constructor_param_names(sub_binding, types);
        let agrees = match (sup_params, sub_params) {
            (None, None) => true,
            (Some(expected), Some(got)) => name_sets_equal(expected, got),
            _ => false,
        };
        if !agrees {
            return Err(SigSubtypeFailure::KindMismatch {
                name,
                expected_params: sup_params,
                got: sub_binding,
            });
        }
        let expected = read(requirement(types, sup_repr));
        let got = read(requirement(types, sub_binding));
        if !is_subtype_of(types, scratch, got, expected) {
            return Err(SigSubtypeFailure::BoundMismatch {
                name,
                got,
                expected,
            });
        }
    }

    // 2. Manifest members: present manifest in `sub` with an equal type, once the requirement is
    // read through `sub`'s bindings — a manifest member may be declared *as* another member
    // (`Wrap = Elt`), and what that fixes it to is whatever `sub` binds `Elt` to.
    for (name, fixed) in sup.manifest_members.iter().copied() {
        let expected = read(fixed);
        match member(sub.manifest_members, name).map(read) {
            Some(got) if got == expected => {}
            Some(got) => {
                return Err(SigSubtypeFailure::ManifestMismatch {
                    name,
                    got,
                    expected,
                });
            }
            // An abstract `sub` member supplies no witness for a manifest requirement.
            None => {
                return Err(match member(sub.abstract_members, name) {
                    Some(repr) => SigSubtypeFailure::ManifestMismatch {
                        name,
                        got: repr,
                        expected,
                    },
                    None => SigSubtypeFailure::MissingTypeMember { name },
                });
            }
        }
    }

    // 3. Value slots: present and covariantly compatible after abstract-member substitution.
    for (name, declared) in sup.value_slots.iter().copied() {
        let Some(carried) = member(sub.value_slots, name) else {
            return Err(SigSubtypeFailure::MissingValueSlot { name });
        };
        let carried = read(carried);
        if !satisfied_by(types, scratch, read(declared), carried) {
            return Err(SigSubtypeFailure::ValueSlotMismatch {
                name,
                got: carried,
                expected: declared,
            });
        }
    }

    // 4. Keyworded members, mirroring dispatch resolution: each declared overload needs at least one
    // satisfying overload in `sub`'s bucket under the same key, and among the satisfiers the most
    // specific is the one it selects. An incomparable tie is a dispatch ambiguity, and rejects here
    // rather than at the call.
    for declared in sup.keyworded.iter().copied() {
        let mut candidates = BumpVec::with_capacity_in(sub.keyworded.len(), scratch);
        candidates.extend(
            sub.keyworded
                .iter()
                .filter(|candidate| shape_keys_equal(declared, **candidate, types))
                .map(|candidate| read(*candidate)),
        );
        // Both sides are read in `sub`'s world, so the selection runs the plain structural rule.
        match select_keyworded_satisfier(types, scratch, read(declared), &candidates, None) {
            Ok(_) => {}
            Err(satisfiers) if satisfiers.is_empty() && candidates.is_empty() => {
                return Err(SigSubtypeFailure::MissingKeyworded { head: declared });
            }
            Err(satisfiers) if satisfiers.is_empty() => {
                if let Some((parameter, got)) =
                    quantified_position_failure(types, scratch, declared, candidates[0])
                {
                    return Err(SigSubtypeFailure::QuantifiedMismatch {
                        head: declared,
                        parameter,
                        got,
                    });
                }
                return Err(SigSubtypeFailure::KeywordedMismatch {
                    head: declared,
                    got: candidates.leak(),
                });
            }
            Err(satisfiers) => {
                return Err(SigSubtypeFailure::AmbiguousKeyworded {
                    head: declared,
                    candidates: scratch
                        .alloc_slice_fill_iter(satisfiers.iter().map(|index| candidates[*index])),
                });
            }
        }
    }

    // 5. Operator members: each declared record needs a `sub` record whose member set **includes**
    // it — width, as for every other channel — under an **equal** mode. At most one `sub` record can
    // cover a declared one: two records in a channel never share a member. Mode is matched exactly
    // because a mode is not an approximation of another — a run folded right and the same run folded
    // left compute different things.
    for declared in sup.operators.iter().copied() {
        let covering = sub
            .operators
            .iter()
            .find(|record| declared.members.iter().all(|m| record.members.contains(m)));
        let Some(covering) = covering else {
            return Err(SigSubtypeFailure::MissingOperatorGroup {
                members: declared.members,
            });
        };
        if covering.mode != declared.mode {
            return Err(SigSubtypeFailure::OperatorModeMismatch {
                members: declared.members,
                expected: declared.mode,
                got: covering.mode,
            });
        }
    }
    Ok(())
}

/// What a type member requires of whatever binds it: a rigid variable's bound, and a manifest
/// binding's own type. The one reading the bound check on both sides of [`sig_subtype`] takes.
fn requirement(types: &TypeRegistry<'_>, member: KType) -> KType {
    match types.node(member) {
        TypeNode::AbstractType { bound, .. } => bound,
        _ => member,
    }
}

/// Why `candidate` failed the declared member, when the reason is a **quantified** position: the
/// first slot whose declared type reads a quantifier and whose candidate slot is not at least as
/// general, as the parameter's name and the type the candidate fixes it to.
fn quantified_position_failure(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    declared: KType,
    candidate: KType,
) -> Option<(TypeSymbol, KType)> {
    let quantifiers = shape_quantifiers(declared, types);
    if quantifiers.is_empty() {
        return None;
    }
    let mut candidate_slots = shape_slots(candidate, types);
    for declared_slot in shape_slots(declared, types) {
        let candidate_slot = candidate_slots.next()?;
        // Contravariance: a candidate position fills a declared one by being equal or more general.
        if is_subtype_of(types, scratch, declared_slot, candidate_slot) {
            continue;
        }
        let named = (0..quantifiers.len())
            .find(|i| types.references_quantifier(scratch, declared_slot, *i))?;
        return Some((quantifiers[named], candidate_slot));
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
/// `Err` carries the satisfier indices, in scratch: empty means nothing satisfied, two or more an
/// incomparable tie.
pub fn select_keyworded_satisfier<'s>(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'s>,
    declared: KType,
    candidates: &[KType],
    substitution: Option<(Members<'_, TypeSymbol>, ScopeId)>,
) -> Result<usize, BumpVec<'s, usize>> {
    let mut satisfiers = BumpVec::with_capacity_in(candidates.len(), scratch);
    satisfiers.extend(
        candidates
            .iter()
            .enumerate()
            .filter(|(_, candidate)| match substitution {
                Some((members, id)) => {
                    slot_satisfied_by(types, scratch, declared, **candidate, id, members)
                }
                None => satisfied_by(types, scratch, declared, **candidate),
            })
            .map(|(index, _)| index),
    );
    if let [only] = satisfiers[..] {
        return Ok(only);
    }
    dominant(satisfiers.len(), |i, j| {
        matches!(
            shape_specificity(
                types,
                scratch,
                candidates[satisfiers[i]],
                candidates[satisfiers[j]]
            ),
            Specificity::StrictlyMore
        )
    })
    .map(|i| satisfiers[i])
    .ok_or(satisfiers)
}

// --- Meet of schemas ---

/// The greatest lower bound of two schemas, interned, or `None` when they make conflicting claims:
/// two manifest bindings for one name, two parameter-name sets for one member, or two chaining
/// modes for one operator run.
///
/// Width unions — a lower bound may promise everything either operand promises — and depth
/// reconciles per member, so a shared value slot meets and a shared type member takes the stronger
/// binding. Both operands' tables are read together in name order, so the result's named tables
/// come out sorted.
pub(super) fn meet_schemas(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    a: SigSchema<'_>,
    b: SigSchema<'_>,
) -> Option<KType> {
    let mut draft = SchemaDraft::new(scratch);
    let left_bindings = a.member_bindings(scratch);
    let right_bindings = b.member_bindings(scratch);
    // Each side's binding for a name is its `type_member` reading: manifest first. The join yields
    // each name once, in order, so the two tables it fills are built from runs already sorted.
    let mut abstract_members = BumpVec::new_in(scratch);
    let mut manifest_members = BumpVec::new_in(scratch);
    for (name, left, right) in merge_join(left_bindings, right_bindings) {
        match (left, right) {
            (Some(left), Some(right)) => {
                let params = match (
                    constructor_param_names(left, types),
                    constructor_param_names(right, types),
                ) {
                    (None, None) => &[][..],
                    (Some(x), Some(y)) if name_sets_equal(x, y) => x,
                    _ => return None,
                };
                let left_manifest = member(a.manifest_members, name).is_some();
                let right_manifest = member(b.manifest_members, name).is_some();
                match (left_manifest, right_manifest) {
                    // A manifest binding is the stronger claim, so it survives — provided it lies
                    // under what the other side requires.
                    (true, true) if left != right => return None,
                    (true, true) => manifest_members.push((name, left)),
                    (true, false) => {
                        if !is_subtype_of(types, scratch, left, requirement(types, right)) {
                            return None;
                        }
                        manifest_members.push((name, left));
                    }
                    (false, true) => {
                        if !is_subtype_of(types, scratch, right, requirement(types, left)) {
                            return None;
                        }
                        manifest_members.push((name, right));
                    }
                    (false, false) => {
                        let bound = meet(
                            types,
                            scratch,
                            requirement(types, left),
                            requirement(types, right),
                        );
                        let merged = types.abstract_type(
                            scratch,
                            ScopeId::SENTINEL,
                            name,
                            params,
                            None,
                            bound,
                        );
                        abstract_members.push((name, merged));
                    }
                }
            }
            (Some(only), None) | (None, Some(only)) => {
                if member(a.abstract_members, name).is_some()
                    || member(b.abstract_members, name).is_some()
                {
                    abstract_members.push((name, only));
                } else {
                    manifest_members.push((name, only));
                }
            }
            (None, None) => unreachable!("a joined name is held by one side"),
        }
    }

    // A shared member takes a binding neither operand's own handle names — the bound is the meet of
    // theirs — so every type carried over from an operand reads its member references through the
    // result's bindings. References are by name, which is what makes that a rename rather than a
    // reinterpretation.
    let abstract_members = Members::from_table(abstract_members);
    let manifest_members = Members::from_table(manifest_members);
    let chosen = merged_bindings(scratch, abstract_members, manifest_members);
    let manifest_members = manifest_members.map_types(scratch, |kt| {
        substitute_sig_members(types, scratch, kt, ScopeId::SENTINEL, chosen)
    });
    let bindings = merged_bindings(scratch, abstract_members, manifest_members);
    let resolve =
        |kt: KType| substitute_sig_members(types, scratch, kt, ScopeId::SENTINEL, bindings);

    for (name, left, right) in merge_join(a.value_slots, b.value_slots) {
        let met = match (left, right) {
            (Some(left), Some(right)) => meet(types, scratch, resolve(left), resolve(right)),
            (Some(only), None) | (None, Some(only)) => resolve(only),
            (None, None) => unreachable!("a joined name is held by one side"),
        };
        draft.value_slots.push((name, met));
    }

    // A lower bound declares every overload either operand does; the signature door's
    // canonicalization then drops whichever of a pair the other already admits.
    for shape in a.keyworded.iter().chain(b.keyworded) {
        draft.keyworded.push(resolve(*shape));
    }

    draft.abstract_members.extend_from_slice(&abstract_members);
    draft.manifest_members.extend_from_slice(&manifest_members);
    draft.operators = merge_operator_records(scratch, a, b)?;
    draft.sig_id = (!abstract_members.is_empty()).then_some(ScopeId::SENTINEL);
    Some(types.signature(scratch, draft))
}

/// Both operands' chaining records, merged so no two records in the result share a member — the
/// invariant a channel carries. Two records sharing a member must agree on the mode; disagreeing is
/// the absence of a meet.
fn merge_operator_records<'s>(
    scratch: BumpAllocator<'s>,
    a: SigSchema<'_>,
    b: SigSchema<'_>,
) -> Option<BumpVec<'s, DeclaredGroup<'s>>> {
    let mut merged: BumpVec<'s, DeclaredGroup<'s>> =
        BumpVec::with_capacity_in(a.operators.len() + b.operators.len(), scratch);
    for record in a.operators.iter().chain(b.operators) {
        let mut overlapping = BumpVec::with_capacity_in(merged.len(), scratch);
        overlapping.extend(
            merged
                .iter()
                .enumerate()
                .filter(|(_, held)| held.members.iter().any(|m| record.members.contains(m)))
                .map(|(index, _)| index),
        );
        if overlapping.iter().any(|i| merged[*i].mode != record.mode) {
            return None;
        }
        let width = record.members.len()
            + overlapping
                .iter()
                .map(|i| merged[*i].members.len())
                .sum::<usize>();
        let mut members = BumpVec::with_capacity_in(width, scratch);
        members.extend_from_slice(record.members);
        for index in overlapping.iter().rev() {
            members.extend_from_slice(merged.remove(*index).members);
        }
        members.sort_unstable();
        members.dedup();
        merged.push(DeclaredGroup {
            members: members.leak(),
            mode: record.mode,
        });
    }
    canonical_groups(&mut merged);
    Some(merged)
}
