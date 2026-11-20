//! Substitution: of a scheme's own variables, of lexical levels, of a signature's head parameters,
//! and of the reads that replace a variable by one of its ends.
//!
//! Every walk here is a [`unary`](super::walk::unary) instance: a leaf rule. Interning is
//! insert-if-absent on a content-addressed table and a substitution that binds nothing returns its
//! input node, so a walk costs one intern per changed composite.
//!
//! Every walk here is over nodes, and so is every binding it substitutes; [`typed`](super::typed)
//! types each one the rest of koan calls.
//!
//! A signature is opaque to every walk here but three: the seal's two sibling walks
//! ([`rewrite_siblings`], [`collect_siblings`]), so a sibling a signature holds is an edge and is
//! rewritten, and a carrier's read-back ([`substitute_carriers`]), so a type over a view's carrier
//! reads back whole. Each enters one through [`TypeChain::signature_mapped`] or its read-only twin,
//! and stops at a node whose stored flag
//! ([`contains_sibling`](super::stored::StoredNode::contains_sibling),
//! [`contains_deep_carrier`](super::stored::StoredNode::contains_deep_carrier)) says nothing it
//! seeks is inside, a signature too.

use crate::symbols::TypeSymbol;
use bumpstorage::{BumpAllocator, BumpVec};

use super::chain::TypeChain;
use super::node::{DerivedId, PathClass, TypeNode, Variable};
use super::schema::{Members, member};
use super::stored::Linked;
use super::tiers::Habitat;
use super::unify::Interval;
use super::walk::Variance;
use super::walk::unary::{Context, Visit, rebuild, visit};

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

/// `kt` with every lexical variable whose level `bindings` binds replaced by its binding, under
/// any scheme — no scheme captures one. What a run reads a load-time type through, each level bound
/// to the type its coordinate holds. A level `bindings` leaves `None`, or past them, is kept, and a
/// signature is opaque.
pub(super) fn substitute_levels<'run>(
    types: &TypeChain<'run, impl Habitat<'run>>,
    scratch: BumpAllocator<'_>,
    kt: Linked<'run>,
    bindings: &[Option<Linked<'run>>],
) -> Linked<'run> {
    if bindings.is_empty() || !kt.contains_rigid() {
        return kt;
    }
    rebuild(types, scratch, kt, &mut |_, node, _| match *node {
        TypeNode::Lexical { level, .. } => bindings.get(level).copied().flatten(),
        _ => None,
    })
}

/// `kt` with every path variable `binding` answers for replaced by its answer, under any scheme —
/// no scheme captures one. What a walk reads a load-time type through its paths' resolutions by.
pub(super) fn substitute_paths<'run>(
    types: &TypeChain<'run, impl Habitat<'run>>,
    scratch: BumpAllocator<'_>,
    kt: Linked<'run>,
    binding: &mut impl FnMut(PathClass, TypeSymbol) -> Option<Linked<'run>>,
) -> Linked<'run> {
    if !kt.contains_rigid() {
        return kt;
    }
    rebuild(types, scratch, kt, &mut |_, node, _| match *node {
        TypeNode::Path { class, name, .. } => binding(class, name),
        _ => None,
    })
}

/// `kt` with every derived name `binding` answers for replaced by its answer, under any scheme —
/// no scheme captures one. What a binding reads a derived name through: the type its term gives,
/// or the name of the term the binding leaves open.
pub(super) fn substitute_derived<'run>(
    types: &TypeChain<'run, impl Habitat<'run>>,
    scratch: BumpAllocator<'_>,
    kt: Linked<'run>,
    binding: &mut impl FnMut(DerivedId) -> Option<Linked<'run>>,
) -> Linked<'run> {
    if !kt.contains_rigid() {
        return kt;
    }
    rebuild(types, scratch, kt, &mut |_, node, _| match *node {
        TypeNode::Derived { term, .. } => binding(term),
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
/// is opaque. A projection is read through its base, which the reader is handed: every step is
/// covariant, so the piece of the base's end is the projection's end. A reader may answer a type
/// that is no binding of the base, which may lack the piece; the projection then reads as its own
/// end, which lies around every binding's piece.
///
/// An application's pin is compared by handle, so it is read by its own rule
/// ([`read_application`]): kept where it reads alike from both sides, else the application is
/// dropped to its other pins from above and is `Never` from below
/// ([solving](../../src/types/solving.md#substitute-then-ask)).
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
    read_through_dyn(types, scratch, kt, side, interval)
}

/// What an interval reader is, behind a `dyn`: [`read_application`] wraps one per pin, which a
/// generic recursion would instantiate without end.
type IntervalOf<'a, 'run> =
    dyn FnMut(Linked<'run>, &TypeNode<'run, Linked<'run>>) -> Option<Interval<Linked<'run>>> + 'a;

/// [`read_through`]'s body, over a `dyn` reader.
fn read_through_dyn<'run>(
    types: &TypeChain<'run, impl Habitat<'run>>,
    scratch: BumpAllocator<'_>,
    kt: Linked<'run>,
    side: Side,
    interval: &mut IntervalOf<'_, 'run>,
) -> Linked<'run> {
    if !kt.contains_rigid() && !kt.contains_carrier() {
        return kt;
    }
    rebuild(types, scratch, kt, &mut |at, node, context| {
        let above = matches!(
            (side, context.variance()),
            (Side::Above, Variance::Co) | (Side::Below, Variance::Contra)
        );
        let free = match *node {
            TypeNode::SignatureApply { .. } | TypeNode::SignatureMeet { .. } => {
                return Some(read_application(
                    types, scratch, at, side, context, interval,
                ));
            }
            // Its base sits at its own position, read by a walk rooted there.
            TypeNode::Projection {
                base,
                step,
                lower,
                bound,
            } => {
                let side = if above { Side::Above } else { Side::Below };
                let base = read_through_dyn(types, scratch, base, side, interval);
                let end = if above { bound } else { lower };
                return Some(types.projection_node(scratch, base, step).unwrap_or(end));
            }
            TypeNode::Quantified { .. } => context.binder_depth() == 0,
            TypeNode::Lexical { .. }
            | TypeNode::Path { .. }
            | TypeNode::Derived { .. }
            | TypeNode::Parameter { .. }
            | TypeNode::Carrier { .. } => true,
            _ => false,
        };
        if !free {
            return None;
        }
        let ends = interval(at, node)?;
        Some(if above { ends.upper } else { ends.lower })
    })
}

/// A signature type `at` read through intervals at `context`. The order compares two pins of one
/// parameter by handle, so each pin is read from both sides: where the two reads agree the pin
/// stays, read; otherwise a read above every instance drops the pin, and a read below every
/// instance is `Never`. A meet so reads each member, and is `Never` where one is.
///
/// Each pin is read by a walk of its own, rooted at the pin, so a `Quantified` an enclosing scheme
/// of `kt` binds is kept rather than read as free.
fn read_application<'run>(
    types: &TypeChain<'run, impl Habitat<'run>>,
    scratch: BumpAllocator<'_>,
    at: Linked<'run>,
    side: Side,
    context: &Context,
    interval: &mut IntervalOf<'_, 'run>,
) -> Linked<'run> {
    if !at.contains_rigid() && !at.contains_carrier() {
        return at;
    }
    let above = matches!(
        (side, context.variance()),
        (Side::Above, Variance::Co) | (Side::Below, Variance::Contra)
    );
    match at.view() {
        TypeNode::SignatureApply { signature, pins } => {
            let bound = context.binder_depth() > 0;
            let mut read = |pin, side| {
                read_through_dyn(types, scratch, pin, side, &mut |at, node| match *node {
                    TypeNode::Quantified { .. } if bound => None,
                    _ => interval(at, node),
                })
            };
            let mut kept = BumpVec::with_capacity_in(pins.len(), scratch);
            for &(name, pin) in pins.raw() {
                let upper = read(pin, Side::Above);
                if upper == read(pin, Side::Below) {
                    kept.push((name, upper));
                } else if !above {
                    return types.never();
                }
            }
            types.signature_apply(scratch, signature, &kept)
        }
        TypeNode::SignatureMeet { members } => {
            let mut read = BumpVec::with_capacity_in(members.len(), scratch);
            for member in members.iter() {
                let member = read_application(types, scratch, member, side, context, interval);
                if member == types.never() {
                    return member;
                }
                read.push(member);
            }
            types.signature_meet(scratch, &read)
        }
        _ => at,
    }
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

/// `kt` with each opaque view's carrier `binding` answers for replaced by its answer — how a view
/// of a view reads a type over its source's carriers on the module both are built over. A carrier
/// is an atom, so the answer lands at the carrier's position alone. `binding` is handed each
/// carrier `kt` holds, so a reader may answer `None` for all of them to list them.
///
/// A signature or an application is entered through
/// [`signature_mapped`](TypeChain::signature_mapped), so a carrier in a slot is read back too. A
/// carrier is a concrete atom no signature binds, so substituting one inside a signature captures
/// nothing.
pub(super) fn substitute_carriers<'run>(
    types: &TypeChain<'run, impl Habitat<'run>>,
    scratch: BumpAllocator<'_>,
    kt: Linked<'run>,
    binding: &mut impl FnMut(Linked<'run>) -> Option<Linked<'run>>,
) -> Linked<'run> {
    if !kt.contains_deep_carrier() {
        return kt;
    }
    rebuild(types, scratch, kt, &mut |at, node, _| match *node {
        _ if !at.contains_deep_carrier() => Some(at),
        TypeNode::Carrier { .. } => binding(at),
        TypeNode::Signature { .. } | TypeNode::SignatureApply { .. } => {
            Some(types.signature_mapped(scratch, at, &mut |slot| {
                substitute_carriers(types, scratch, slot, binding)
            }))
        }
        _ => None,
    })
}

/// Deep-rewrite every [`TypeNode::Sibling`] in `kt` through `resolve`, re-interning each composite
/// on the way out. A sealed member is a leaf, so a cyclic edge into already-sealed content
/// terminates rather than descending forever. Every member `resolve` names is interned before the
/// rewrite, so a union re-interns through the canonical door, reading member nodes that exist.
///
/// A signature or an application is entered through
/// [`signature_mapped`](TypeChain::signature_mapped), which re-interns it canonical: a relative
/// schema may hold a signature over siblings. A node holding no sibling is kept whole, unentered.
pub(super) fn rewrite_siblings<'run>(
    types: &TypeChain<'run, impl Habitat<'run>>,
    scratch: BumpAllocator<'_>,
    kt: Linked<'run>,
    resolve: &impl Fn(usize) -> Linked<'run>,
) -> Linked<'run> {
    rebuild(types, scratch, kt, &mut |at, node, _| match *node {
        _ if !at.contains_sibling() => Some(at),
        TypeNode::Sibling(index) => Some(resolve(index)),
        TypeNode::Signature { .. } | TypeNode::SignatureApply { .. } => {
            Some(types.signature_mapped(scratch, at, &mut |slot| {
                rewrite_siblings(types, scratch, slot, resolve)
            }))
        }
        _ => None,
    })
}

/// Collect every sibling index `kt` references, at any depth, in walk order — inside a signature
/// or an application too, entered through
/// [`signature_handles`](TypeChain::signature_handles), as [`rewrite_siblings`] enters one. A node
/// holding no sibling is skipped, unentered.
pub(super) fn collect_siblings<'run>(
    types: &TypeChain<'run, impl Habitat<'run>>,
    scratch: BumpAllocator<'_>,
    kt: Linked<'run>,
    out: &mut BumpVec<'_, usize>,
) {
    visit(scratch, kt, &mut |at, node, _| match *node {
        _ if !at.contains_sibling() => Visit::Skip,
        TypeNode::Sibling(index) => {
            out.push(index);
            Visit::Skip
        }
        TypeNode::Signature { .. } | TypeNode::SignatureApply { .. } => {
            types.signature_handles(at, &mut |slot| collect_siblings(types, scratch, slot, out));
            Visit::Skip
        }
        _ => Visit::Descend,
    });
}
