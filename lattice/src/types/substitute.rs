//! Substitution: of a scheme's own variables, of lexical levels, of a signature's head parameters,
//! and of the reads that replace a variable by one of its ends.
//!
//! Every walk here is a [`unary`](super::walk::unary) instance: a leaf rule. Interning is
//! insert-if-absent on a content-addressed table and a substitution that binds nothing returns its
//! input node, so a walk costs one intern per changed composite.
//!
//! Every walk here is over nodes, and so is every binding it substitutes; [`typed`](super::typed)
//! types each one the rest of koan calls.

use crate::bump::{BumpAllocator, BumpVec};
use crate::symbols::TypeSymbol;

use super::chain::TypeChain;
use super::node::{TypeNode, Variable};
use super::schema::{Members, member};
use super::stored::Linked;
use super::tiers::Habitat;
use super::unify::Interval;
use super::walk::Variance;
use super::walk::unary::{Visit, rebuild, visit};

/// Rewrite every **free** `Quantified(i)` inside `kt` to `bindings[i]` — the per-call substitution
/// a solved call applies to a shape's return.
///
/// A **nested** scheme rebinds the indices with its own group, exactly as it shadows them in the
/// relations, so a variable under one is not free and is left alone.
pub(super) fn substitute_quantified<'run>(
    types: &TypeChain<'run, impl Habitat<'run>>,
    scratch: BumpAllocator<'_>,
    kt: Linked<'run>,
    bindings: &[Linked<'run>],
) -> Linked<'run> {
    if bindings.is_empty() {
        return kt;
    }
    rebuild(types, scratch, kt, &mut |kt, node, context| match *node {
        TypeNode::Quantified { index, .. } if context.binder_depth() == 0 => {
            Some(bindings.get(index).copied().unwrap_or(kt))
        }
        _ => None,
    })
}

/// `kt` with every lexical variable whose level `bindings` covers replaced by its binding, under
/// any scheme — no scheme captures one. What a run reads a load-time type through, each level bound
/// to the type its coordinate holds. A level past `bindings` is kept, and a signature is opaque.
pub(super) fn substitute_levels<'run>(
    types: &TypeChain<'run, impl Habitat<'run>>,
    scratch: BumpAllocator<'_>,
    kt: Linked<'run>,
    bindings: &[Linked<'run>],
) -> Linked<'run> {
    if bindings.is_empty() || !kt.contains_rigid() {
        return kt;
    }
    rebuild(types, scratch, kt, &mut |_, node, _| match *node {
        TypeNode::Lexical { level, .. } => bindings.get(level).copied(),
        _ => None,
    })
}

/// `kt` at a solved call. A scheme's body has the scheme's variables free, so instantiating a
/// scheme substitutes its body through `bindings` and answers the callable with no group — the
/// callable this call has, as against the one the declaration wrote. Any other type substitutes in
/// place.
///
/// `bindings` are in the scheme's **group** order — a caller holding declaration-order bindings
/// translates them through the map [`function_scheme`](super::chain::TypeChain::function_scheme) or
/// [`shape_scheme`](super::chain::TypeChain::shape_scheme) handed back.
pub(super) fn instantiate_quantified<'run>(
    types: &TypeChain<'run, impl Habitat<'run>>,
    scratch: BumpAllocator<'_>,
    kt: Linked<'run>,
    bindings: &[Linked<'run>],
) -> Linked<'run> {
    substitute_quantified(types, scratch, kt.split().1, bindings)
}

/// `kt` with every rigid variable reachable from it replaced by its bound — the
/// variable-free type it constrains to. An opaque carrier is concrete, so it is kept.
///
/// A bound is itself variable-free, so one pass reaches a fixed point. What a caller minting a
/// *bound* out of an arbitrary type runs it through, since the two doors that take one require it.
pub(super) fn erase_rigid<'run>(
    types: &TypeChain<'run, impl Habitat<'run>>,
    scratch: BumpAllocator<'_>,
    kt: Linked<'run>,
) -> Linked<'run> {
    rebuild(types, scratch, kt, &mut |_, node, _| node.rigid_bound())
}

/// `kt` with each free variable — a `Quantified` under none of `kt`'s own schemes, a `Lexical` or
/// a `Parameter` — read as the extreme that puts the result above every instance within the
/// variables' ends: its bound at a covariant position, its lower end at a contravariant one. The
/// variable-free type a load-time type is compared through where the run may bind its variables to
/// anything under their bounds. A signature is opaque, as
/// [`StoredNode::contains_rigid`](super::stored::StoredNode::contains_rigid) reads it, and an opaque
/// carrier is concrete, so it is kept.
///
/// [`erase_rigid`] reads a variable as its bound everywhere, which at a contravariant position puts
/// the result *below* an instance whose variable is bound lower.
pub(super) fn bound_above<'run>(
    types: &TypeChain<'run, impl Habitat<'run>>,
    scratch: BumpAllocator<'_>,
    kt: Linked<'run>,
) -> Linked<'run> {
    read_through(types, scratch, kt, Side::Above, &mut |_, node| {
        Some(Variable::of(node)?.interval_in(types))
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
/// `interval` is handed the node and its view, so a reader can answer with the node itself as an
/// end. A carrier is concrete, and only a signature's fit reads one so, as an interval up to the
/// bound its view's source met ([`Collector::heads`](super::unify::Collector::heads)); every other
/// reader answers `None` for it.
pub(super) fn read_through<'run>(
    types: &TypeChain<'run, impl Habitat<'run>>,
    scratch: BumpAllocator<'_>,
    kt: Linked<'run>,
    side: Side,
    interval: &mut impl FnMut(
        Linked<'run>,
        &TypeNode<'run, Linked<'run>>,
    ) -> Option<Interval<Linked<'run>>>,
) -> Linked<'run> {
    if !kt.contains_rigid() && !kt.contains_carrier() {
        return kt;
    }
    rebuild(types, scratch, kt, &mut |at, node, context| {
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
pub(super) fn substitute_parameters<'run>(
    types: &TypeChain<'run, impl Habitat<'run>>,
    scratch: BumpAllocator<'_>,
    kt: Linked<'run>,
    bindings: Members<'_, TypeSymbol, Linked<'run>>,
) -> Linked<'run> {
    if bindings.is_empty() || !kt.contains_rigid() {
        return kt;
    }
    rebuild(types, scratch, kt, &mut |_, node, _| match *node {
        TypeNode::Parameter { name, .. } => member(bindings, name),
        _ => None,
    })
}

/// Deep-rewrite every [`TypeNode::Sibling`] in `kt` through `resolve`, re-interning each composite
/// on the way out. A sealed member is a leaf, so a cyclic edge into already-sealed content
/// terminates rather than descending forever. Every member `resolve` names is interned before the
/// rewrite, so a union re-interns through the canonical door, reading member nodes that exist.
pub(super) fn rewrite_siblings<'run>(
    types: &TypeChain<'run, impl Habitat<'run>>,
    scratch: BumpAllocator<'_>,
    kt: Linked<'run>,
    resolve: &impl Fn(usize) -> Linked<'run>,
) -> Linked<'run> {
    rebuild(types, scratch, kt, &mut |_, node, _| match *node {
        TypeNode::Sibling(index) => Some(resolve(index)),
        _ => None,
    })
}

/// Collect every sibling index `kt` references, at any depth, in walk order.
pub(super) fn collect_siblings(
    scratch: BumpAllocator<'_>,
    kt: Linked<'_>,
    out: &mut BumpVec<'_, usize>,
) {
    visit(scratch, kt, &mut |_, node, _| match *node {
        TypeNode::Sibling(index) => {
            out.push(index);
            Visit::Skip
        }
        _ => Visit::Descend,
    });
}
