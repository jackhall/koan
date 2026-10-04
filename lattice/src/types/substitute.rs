//! Substitution: of a binder's own variables, of lexical levels, of a signature's head parameters,
//! and of the reads that replace a variable by one of its ends.
//!
//! Every walk here is a [`unary`](super::walk::unary) instance: a leaf rule plus the union door.
//! Interning is insert-if-absent on a content-addressed table and a substitution that binds nothing
//! returns its input handle, so a walk costs one intern per changed composite.
//!
//! Every walk here is over raw handles; [`typed`](super::typed) types each one the rest of koan
//! calls.

use crate::bump::{BumpAllocator, BumpVec};
use crate::symbols::TypeSymbol;

use super::handle::{Handle, KType, TypeHandle};
use super::node::{TypeNode, Variable};
use super::record::map_fields;
use super::registry::TypeRegistry;
use super::schema::{Members, member};
use super::shape::map_slots;
use super::unify::Interval;
use super::walk::Variance;
use super::walk::unary::{Rebuild, UnionDoor, Visit, rebuild, visit};

/// The knobs every walk here but the seal's takes: rebuilt unions canonicalize.
const CANONICAL: Rebuild = Rebuild {
    union: UnionDoor::Canonical,
};

/// Rewrite every **free** `Quantified(i)` inside `kt` to `bindings[i]` — the per-call substitution
/// a solved call applies to a shape's return.
///
/// A **nested** binder — a shape, or a function type carrying a group — rebinds the indices with
/// its own group, exactly as it shadows them in the relations, so a variable under one is not free
/// and is left alone.
pub(super) fn substitute_quantified<B: TypeHandle>(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    kt: Handle,
    bindings: &[B],
) -> Handle {
    if bindings.is_empty() {
        return kt;
    }
    rebuild(
        types,
        scratch,
        kt,
        CANONICAL,
        &mut |kt, node, context| match *node {
            TypeNode::Quantified { index, .. } if context.binder_depth() == 0 => {
                Some(bindings.get(index).map_or(kt, |binding| binding.raw()))
            }
            _ => None,
        },
    )
}

/// `kt` with every lexical variable whose level `bindings` covers replaced by its binding, under
/// any binder — no binder captures one. What a run reads a load-time type through, each level bound
/// to the type its coordinate holds. A level past `bindings` is kept, and a signature is opaque.
pub(super) fn substitute_levels<B: TypeHandle>(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    kt: Handle,
    bindings: &[B],
) -> Handle {
    if bindings.is_empty() || !types.contains_rigid(kt) {
        return kt;
    }
    rebuild(
        types,
        scratch,
        kt,
        CANONICAL,
        &mut |_, node, _| match *node {
            TypeNode::Lexical { level, .. } => bindings.get(level).map(|binding| binding.raw()),
            _ => None,
        },
    )
}

/// `kt` at a solved call. A **binder** — a shape, or a function type carrying a group — owns the
/// variables, so instantiating one opens it: each position and the return, read on its own, has
/// the binder's variables free, and substitutes through `bindings`; the result is interned with no
/// group, the callable this call has as against the one the declaration wrote. Every other type
/// carries no binder of its own and substitutes in place.
///
/// `bindings` are in the binder's **group** order — a caller holding declaration-order bindings
/// translates them through the map [`shape_type`](TypeRegistry::shape_type) or
/// [`function_type`](TypeRegistry::function_type) handed back.
pub(super) fn instantiate_quantified<B: TypeHandle>(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    kt: Handle,
    bindings: &[B],
) -> Handle {
    if quantifier_bounds(types, kt).is_empty() {
        return substitute_quantified(types, scratch, kt, bindings);
    }
    let open = |position: Handle| substitute_quantified(types, scratch, position, bindings);
    match types.node(kt) {
        TypeNode::KFunction { params, ret, .. } => {
            types.function_type(scratch, &map_fields(scratch, params.raw(), open), open(ret))
        }
        TypeNode::ExpressionShape {
            elements,
            classes,
            ret,
            ..
        } => types.shape_type(
            scratch,
            &map_slots(scratch, elements.raw(), open),
            classes,
            open(ret),
        ),
        _ => unreachable!("only a binder carries a group"),
    }
}

/// `kt` with every rigid variable reachable from it replaced by its bound — the
/// variable-free type it constrains to.
///
/// A bound is itself variable-free, so one pass reaches a fixed point. What a caller minting a
/// *bound* out of an arbitrary type runs it through, since the two doors that take one require it.
pub(super) fn erase_rigid(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    kt: Handle,
) -> Handle {
    rebuild(types, scratch, kt, CANONICAL, &mut |_, node, _| {
        node.rigid_bound().map(KType::raw)
    })
}

/// `kt` with each free variable — a `Quantified` under none of `kt`'s own binders, a `Lexical` or
/// a `Parameter` — read as the extreme that puts the result above every instance within the
/// variables' ends: its bound at a covariant position, its lower end at a contravariant one. The
/// variable-free type a load-time type is compared through where the run may bind its variables to
/// anything under their bounds. A signature is opaque, as [`TypeRegistry::contains_rigid`] reads it.
///
/// [`erase_rigid`] reads a variable as its bound everywhere, which at a contravariant position puts
/// the result *below* an instance whose variable is bound lower.
pub(super) fn bound_above(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    kt: Handle,
) -> Handle {
    read_through(types, scratch, kt, Side::Above, &mut |variable| {
        Some(variable.interval().raw())
    })
}

/// Which extreme a read through intervals takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// Above every instance: a variable's upper end at a covariant position, its lower end at a
    /// contravariant one.
    Above,
    /// Below every instance: its lower end at a covariant position, its upper end at a
    /// contravariant one.
    Below,
}

/// `kt` read through intervals: each free variable — a `Quantified` under none of `kt`'s own
/// binders, a lexical variable, a `Parameter` — that `interval` answers for replaced by the end
/// `side` takes at its position; one it answers `None` for is kept. A signature is opaque.
pub(super) fn read_through(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    kt: Handle,
    side: Side,
    interval: &mut impl FnMut(Variable) -> Option<Interval<Handle>>,
) -> Handle {
    if !types.contains_rigid(kt) {
        return kt;
    }
    rebuild(types, scratch, kt, CANONICAL, &mut |_, node, context| {
        let free = match *node {
            TypeNode::Quantified { .. } => context.binder_depth() == 0,
            TypeNode::Lexical { .. } | TypeNode::Parameter { .. } => true,
            _ => false,
        };
        if !free {
            return None;
        }
        let ends = interval(Variable::of(node)?)?;
        Some(match (side, context.variance()) {
            (Side::Above, Variance::Co) | (Side::Below, Variance::Contra) => ends.upper,
            _ => ends.lower,
        })
    })
}

/// The bound of each variable of `kt`'s own quantifier group, in group order,
/// as the binder node stores it. Empty for anything that binds no group.
pub(super) fn quantifier_bounds<'run>(types: &TypeRegistry<'run>, kt: Handle) -> &'run [KType] {
    types.node(kt).group().map_or(&[], |(_, bounds)| bounds)
}

/// `kt` with each head parameter `bindings` names replaced by its binding — how a signature's
/// member types are read under a module's solution, a view's mints or an application's pins.
///
/// A parameter is matched by name, and only a nonce-free one: a nonced `Parameter` is an opaque
/// view's mint, which no declaration names. A `Signature` node is opaque — its own parameters are
/// its own, and a declared signature is closed — so the walk reaches only an application's pins.
pub(super) fn substitute_parameters<B: TypeHandle>(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    kt: Handle,
    bindings: Members<'_, TypeSymbol, B>,
) -> Handle {
    if bindings.is_empty() || !types.contains_rigid(kt) {
        return kt;
    }
    rebuild(
        types,
        scratch,
        kt,
        CANONICAL,
        &mut |_, node, _| match *node {
            TypeNode::Parameter {
                name, nonce: None, ..
            } => member(bindings, name).map(TypeHandle::raw),
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
    kt: Handle,
    resolve: &impl Fn(usize) -> KType,
) -> Handle {
    rebuild(
        types,
        scratch,
        kt,
        Rebuild {
            union: UnionDoor::Flat,
        },
        &mut |_, node, _| match *node {
            TypeNode::Sibling(index) => Some(resolve(index).raw()),
            _ => None,
        },
    )
}

/// Collect every sibling index `kt` references, at any depth, in walk order.
pub(super) fn collect_siblings(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    kt: Handle,
    out: &mut BumpVec<'_, usize>,
) {
    visit(types, scratch, kt, &mut |_, node, _| match *node {
        TypeNode::Sibling(index) => {
            out.push(index);
            Visit::Skip
        }
        _ => Visit::Descend,
    });
}
