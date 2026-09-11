//! Substitution, and the relations that are a substitution composed with an ordinary one.
//!
//! Every walk here is a [`unary`](super::walk::unary) instance: a leaf rule plus descent knobs.
//! The compositions at the bottom are the whole of "substitute, then ask" — no second structural
//! descent exists for any of them. Interning is insert-if-absent on a content-addressed table, a
//! substitution that binds nothing returns its input handle, and every subtype verdict is
//! memoized, so the composition costs one intern per changed composite.

use crate::memory::{BumpAllocator, BumpVec, ScopeId};
use crate::parse::TypeSymbol;

use super::handle::KType;
use super::node::TypeNode;
use super::order::is_subtype_of;
use super::registry::TypeRegistry;
use super::schema::{Members, member};
use super::walk::unary::{LEAF, Rebuild, Step, UnionDoor, Visit, rebuild, visit};

/// The knobs a quantifier walk takes: a nested signature is opaque content, and rebuilt unions
/// canonicalize.
const OVER_QUANTIFIERS: Rebuild = Rebuild {
    signature: Step::Leaf,
    union: UnionDoor::Canonical,
};

/// The knobs a member walk takes: a nested signature is descended, because a slot type inside one
/// can reference the enclosing binder's members.
const OVER_MEMBERS: Rebuild = Rebuild {
    signature: Step::Through,
    union: UnionDoor::Canonical,
};

/// Rewrite every **free** `Quantified(i)` inside `kt` to `bindings[i]` — the per-call substitution
/// a solved call applies to a shape's return, and what erasure runs with each variable's bound in
/// every cell.
///
/// A **nested** shape rebinds the indices with its own group, exactly as it shadows them in the
/// relations, so a variable under one is not free and is left alone.
pub fn substitute_quantified(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    kt: KType,
    bindings: &[KType],
) -> KType {
    if bindings.is_empty() {
        return kt;
    }
    rebuild(
        types,
        scratch,
        kt,
        OVER_QUANTIFIERS,
        &mut |kt, node, context| match *node {
            TypeNode::Quantified { index, .. } if context.shape_depth() == 0 => {
                Some(bindings.get(index).copied().unwrap_or(kt))
            }
            _ => None,
        },
    )
}

/// `kt` at a solved call. A **shape** is its own binder, so instantiating one substitutes through
/// its slots and return and the canonical form drops the emptied group: the result is the shape
/// this call has, as against the shape the declaration wrote. Every other type carries no binder of
/// its own and substitutes in place.
///
/// `bindings` are in the shape's **canonical** quantifier order — a caller holding
/// declaration-order bindings translates them through the map
/// [`shape_type`](TypeRegistry::shape_type) handed back.
pub fn instantiate_quantified(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    kt: KType,
    bindings: &[KType],
) -> KType {
    let is_shape = matches!(
        types.node(kt),
        TypeNode::ExpressionShape { quantifiers, .. } if !quantifiers.is_empty()
    );
    if !is_shape {
        return substitute_quantified(types, scratch, kt, bindings);
    }
    rebuild(
        types,
        scratch,
        kt,
        OVER_QUANTIFIERS,
        &mut |kt, node, context| match *node {
            TypeNode::Quantified { index, .. } if context.shape_depth() == 1 => {
                Some(bindings.get(index).copied().unwrap_or(kt))
            }
            _ => None,
        },
    )
}

/// `kt` with every variable of its own group replaced by that variable's bound — what a quantified
/// callable reports on the value lane, where a lambda type has no binder to carry the parameter.
pub fn erase_quantified(types: &TypeRegistry<'_>, scratch: BumpAllocator<'_>, kt: KType) -> KType {
    let bounds = quantifier_bounds(types, kt);
    if bounds.is_empty() {
        return kt;
    }
    instantiate_quantified(types, scratch, kt, bounds)
}

/// `kt` with every rigid variable reachable from it replaced by the bound it stands over — the
/// variable-free type it constrains to.
///
/// A bound is itself variable-free, so one pass reaches a fixed point. What a caller minting a
/// *bound* out of an arbitrary type runs it through, since the two doors that take one require it.
pub fn erase_rigid(types: &TypeRegistry<'_>, scratch: BumpAllocator<'_>, kt: KType) -> KType {
    rebuild(
        types,
        scratch,
        kt,
        OVER_MEMBERS,
        &mut |_, node, _| match *node {
            TypeNode::Quantified { bound, .. } | TypeNode::AbstractType { bound, .. } => {
                Some(bound)
            }
            _ => None,
        },
    )
}

/// The bound each variable of `kt`'s own quantifier group stands over, in canonical index order,
/// as the shape node stores it. Empty for anything that is not a quantified shape.
pub fn quantifier_bounds<'run>(types: &TypeRegistry<'run>, kt: KType) -> &'run [KType] {
    match types.node(kt) {
        TypeNode::ExpressionShape { bounds, .. } => bounds,
        _ => &[],
    }
}

/// Rewrite `kt`, replacing references to `sig_id`'s abstract members with the caller's bindings for
/// them.
///
/// One reference shape substitutes: a nonce-free `AbstractType { source: sig_id, name }` of either
/// order — a first-order slot type, or a higher-kinded member in the constructor position of a
/// `ConstructorApply`. A nonced `AbstractType` is an opaque ascription's generative mint, not a
/// reference to a declaration, so it never substitutes even when it shares its binder's `source`
/// and name.
///
/// A **nested `Signature`** is descended like any other compound: a signature standing in a slot
/// type carries references to the enclosing signature's members. The nested schema's *own* abstract
/// members shadow the enclosing binder **by name**, which the driver's context reports.
pub fn substitute_sig_members(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    kt: KType,
    sig_id: ScopeId,
    members: Members<'_, TypeSymbol>,
) -> KType {
    if members.is_empty() {
        return kt;
    }
    rebuild(
        types,
        scratch,
        kt,
        OVER_MEMBERS,
        &mut |_, node, context| match *node {
            TypeNode::AbstractType {
                source,
                name,
                nonce: None,
                ..
            } if source == sig_id && !context.shadows(name) => member(members, name),
            _ => None,
        },
    )
}

/// Rewrite every reference to `declared`'s own abstract members so it is sourced at
/// [`ScopeId::SENTINEL`] instead — the canonical binder every projected SIG schema shares.
///
/// Structurally this is [`substitute_sig_members`] with the substitution being a re-source rather
/// than a lookup, and with no shadow subtraction: a nested projected SIG's members were already
/// re-sourced when that SIG was projected, so `source == declared` can only match a genuine outer
/// reference. Running it at projection is what makes two textually identical declarations one
/// interned type.
pub fn canonicalize_binder(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    kt: KType,
    declared: ScopeId,
) -> KType {
    rebuild(
        types,
        scratch,
        kt,
        OVER_MEMBERS,
        &mut |_, node, _| match *node {
            TypeNode::AbstractType {
                source,
                name,
                param_names,
                nonce: None,
                bound,
            } if source == declared => Some(types.abstract_type(
                scratch,
                ScopeId::SENTINEL,
                name,
                param_names,
                None,
                canonicalize_binder(types, scratch, bound, declared),
            )),
            _ => None,
        },
    )
}

/// Deep-rewrite every [`TypeNode::Sibling`] in `kt` through `resolve`, re-interning each composite
/// on the way out. A sealed member handle is a leaf, so a cyclic edge into already-sealed content
/// terminates rather than descending forever.
///
/// Unions re-intern through the **flat** door: a rewritten sibling handle names a still-uninterned
/// member of the group being sealed, which the canonicalizing door would fault on reading — and the
/// rename preserves every subsumption verdict anyway, so the result is canonical without a second
/// pass.
pub(super) fn rewrite_siblings(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    kt: KType,
    resolve: &impl Fn(usize) -> KType,
) -> KType {
    rebuild(
        types,
        scratch,
        kt,
        Rebuild {
            signature: Step::Leaf,
            union: UnionDoor::Flat,
        },
        &mut |_, node, _| match *node {
            TypeNode::Sibling(index) => Some(resolve(index)),
            _ => None,
        },
    )
}

/// Collect every sibling index `kt` references, at any depth, in walk order.
pub(super) fn collect_siblings(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    kt: KType,
    out: &mut BumpVec<'_, usize>,
) {
    visit(types, scratch, kt, LEAF, &mut |_, node, _| match *node {
        TypeNode::Sibling(index) => {
            out.push(index);
            Visit::Skip
        }
        _ => Visit::Descend,
    });
}

// --- Substitute, then ask ---
//
// Each of the three is the member substitution composed with one ordinary relation. The guard set
// of the order is stated once, in `order.rs`, and reached from here through `is_subtype_of`.

/// Whether `carried` satisfies the declared slot type once `sig_id`'s members are read through
/// `members`.
pub fn slot_satisfied_by(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    declared: KType,
    carried: KType,
    sig_id: ScopeId,
    members: Members<'_, TypeSymbol>,
) -> bool {
    let substituted = substitute_sig_members(types, scratch, declared, sig_id, members);
    is_subtype_of(types, scratch, carried, substituted)
}

/// Whether the substituted declared type is at or below `other`.
pub fn slot_more_specific_or_equal(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    declared: KType,
    other: KType,
    sig_id: ScopeId,
    members: Members<'_, TypeSymbol>,
) -> bool {
    let substituted = substitute_sig_members(types, scratch, declared, sig_id, members);
    is_subtype_of(types, scratch, substituted, other)
}

/// Whether the substituted declared type is `other` exactly. Handle equality: two interned types
/// are equal iff their content is.
pub fn slot_types_equal(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    declared: KType,
    other: KType,
    sig_id: ScopeId,
    members: Members<'_, TypeSymbol>,
) -> bool {
    substitute_sig_members(types, scratch, declared, sig_id, members) == other
}
