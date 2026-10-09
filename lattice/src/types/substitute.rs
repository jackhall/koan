//! Substitution: of a scheme's own variables, of lexical levels, of a signature's head parameters,
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
use super::registry::TypeRegistry;
use super::schema::{Members, member};
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
/// A **nested** scheme rebinds the indices with its own group, exactly as it shadows them in the
/// relations, so a variable under one is not free and is left alone.
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
/// any scheme — no scheme captures one. What a run reads a load-time type through, each level bound
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

/// `kt` at a solved call. A scheme's body has the scheme's variables free, so instantiating a
/// scheme substitutes its body through `bindings` and answers the callable with no group — the
/// callable this call has, as against the one the declaration wrote. Any other type substitutes in
/// place.
///
/// `bindings` are in the scheme's **group** order — a caller holding declaration-order bindings
/// translates them through the map [`function_scheme`](TypeRegistry::function_scheme) or
/// [`shape_scheme`](TypeRegistry::shape_scheme) handed back.
pub(super) fn instantiate_quantified<B: TypeHandle>(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    kt: Handle,
    bindings: &[B],
) -> Handle {
    substitute_quantified(types, scratch, types.split(kt).1, bindings)
}

/// `kt` with every rigid variable reachable from it replaced by its bound — the
/// variable-free type it constrains to. An opaque carrier is concrete, so it is kept.
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

/// `kt` with each free variable — a `Quantified` under none of `kt`'s own schemes, a `Lexical` or
/// a `Parameter` — read as the extreme that puts the result above every instance within the
/// variables' ends: its bound at a covariant position, its lower end at a contravariant one. The
/// variable-free type a load-time type is compared through where the run may bind its variables to
/// anything under their bounds. A signature is opaque, as [`TypeRegistry::contains_rigid`] reads it,
/// and an opaque carrier is concrete, so it is kept.
///
/// [`erase_rigid`] reads a variable as its bound everywhere, which at a contravariant position puts
/// the result *below* an instance whose variable is bound lower.
pub(super) fn bound_above(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    kt: Handle,
) -> Handle {
    read_through(types, scratch, kt, Side::Above, &mut |_, node| {
        Some(Variable::of(node)?.interval().raw())
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
/// schemes, a lexical variable, a `Parameter` — and each opaque carrier that `interval` answers for
/// replaced by the end `side` takes at its position; one it answers `None` for is kept. A signature
/// is opaque.
///
/// `interval` is handed the node and its handle, so a reader can answer with the handle itself as
/// an end. A carrier is concrete, and only a signature's fit reads one so, as an interval up to the
/// bound its view's source met ([`Collector::heads`](super::unify::Collector::heads)); every other
/// reader answers `None` for it.
pub(super) fn read_through(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    kt: Handle,
    side: Side,
    interval: &mut impl FnMut(Handle, &TypeNode<'_>) -> Option<Interval<Handle>>,
) -> Handle {
    if !types.contains_rigid(kt) && !types.contains_carrier(kt) {
        return kt;
    }
    rebuild(types, scratch, kt, CANONICAL, &mut |at, node, context| {
        let free = match *node {
            TypeNode::Quantified { .. } => context.binder_depth() == 0,
            TypeNode::Lexical { .. } | TypeNode::Parameter { .. } | TypeNode::Carrier { .. } => {
                true
            }
            _ => false,
        };
        if !free {
            return None;
        }
        let ends = interval(at, node)?;
        Some(match (side, context.variance()) {
            (Side::Above, Variance::Co) | (Side::Below, Variance::Contra) => ends.upper,
            _ => ends.lower,
        })
    })
}

/// `kt` with each head parameter `bindings` names replaced by its binding — how a signature's
/// member types are read under a module's solution, a view's mints or an application's pins.
///
/// A parameter is matched by name; an opaque view's carrier is a node of its own, which no
/// declaration names. A `Signature` node is opaque — its own parameters are its own, and a declared
/// signature is closed — so the walk reaches only an application's pins.
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
            TypeNode::Parameter { name, .. } => member(bindings, name).map(TypeHandle::raw),
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
