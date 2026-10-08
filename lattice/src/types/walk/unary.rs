//! The unary driver: one arm table, three entry points.
//!
//! [`children`] lists a compound node's child handles in a fixed order, each with the variance its
//! position carries; [`reassemble`] puts a node back together from replacement children in that
//! same order. Every structural recursion over one type is [`visit`] or [`rebuild`] over that
//! pair, so a new compound variant is a compile error in exactly two matches here.
//! [`visit_free_quantified`] is [`visit`] narrowed to the free `Quantified` positions, the probe
//! every binder check shares.
//!
//! A signature's members and a sealed member's schema are closed content: neither walk reaches
//! inside one.
//!
//! Both entry points take the scratch allocator the walk's transient buffers — each node's child
//! list and the rebuilt children — are built in.

use crate::bump::{BumpAllocator, BumpVec};

use super::Variance;
use crate::types::handle::{Handle, TypeHandle, wrap};
use crate::types::node::TypeNode;
use crate::types::record::map_fields;
use crate::types::registry::TypeRegistry;
use crate::types::shape::{DispatchTokenElement, map_slots};

/// What a [`visit`] rule does at the node it was handed.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Visit {
    /// Walk this node's children.
    Descend,
    /// Leave this subtree alone and carry on with the rest of the walk.
    Skip,
    /// End the whole walk here.
    Stop,
}

/// Which union door reassembles a rebuilt union.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum UnionDoor {
    /// [`TypeRegistry::union_of`] — the canonicalizing door, which drops a member below another.
    Canonical,
    /// [`TypeRegistry::intern_union_flat`] — dedup and collapse without reading member nodes. The
    /// seal's door, and the seal's alone: a rewritten sibling handle names a member the registry
    /// has not interned yet, so a canonicalizing pass would fault on it.
    Flat,
}

/// [`rebuild`]'s knobs.
#[derive(Clone, Copy)]
pub struct Rebuild {
    pub union: UnionDoor,
}

/// What the driver knows about the position a rule is looking at.
pub struct Context {
    binder_depth: usize,
    variance: Variance,
}

impl Context {
    fn root(variance: Variance) -> Self {
        Context {
            binder_depth: 0,
            variance,
        }
    }

    /// Binders — expression shapes, and function types carrying a group — on the path from the
    /// root to here, the root included: `0` means a [`TypeNode::Quantified`] seen here is
    /// **free**, `1` that it is bound by the root binder's own group.
    pub fn binder_depth(&self) -> usize {
        self.binder_depth
    }

    /// Polarity of the current position: flipped once per enclosing function parameter or shape
    /// slot, [`Variance::Co`] at the root.
    pub fn variance(&self) -> Variance {
        self.variance
    }
}

/// Walk `root` pre-order, handing every node reached to `at`. Returns whether the walk stopped
/// early — the verdict a "does this type contain …" probe reads.
pub fn visit<'run>(
    types: &TypeRegistry<'run>,
    scratch: BumpAllocator<'_>,
    root: Handle,
    at: &mut impl FnMut(Handle, &TypeNode<'run>, &Context) -> Visit,
) -> bool {
    visit_in(types, scratch, root, Variance::Co, at)
}

/// [`visit`] with the root's own polarity supplied — what a walk over a position already known to
/// be contravariant takes, such as the occurrence census over one argument slot of a shape being
/// interned, which has no shape node above it yet to flip the variance for it.
pub fn visit_in<'run>(
    types: &TypeRegistry<'run>,
    scratch: BumpAllocator<'_>,
    root: Handle,
    variance: Variance,
    at: &mut impl FnMut(Handle, &TypeNode<'run>, &Context) -> Visit,
) -> bool {
    let mut context = Context::root(variance);
    visit_at(types, scratch, root, &mut context, at)
}

/// [`visit_in`] over the **free** `Quantified` positions of `root` alone: a nested binder's group
/// is skipped whole, every other node is descended, and each free variable's index is handed to
/// `at` with its context. Returns whether `at` stopped the walk.
pub fn visit_free_quantified<'run>(
    types: &TypeRegistry<'run>,
    scratch: BumpAllocator<'_>,
    root: Handle,
    variance: Variance,
    at: &mut impl FnMut(usize, &Context) -> Visit,
) -> bool {
    visit_in(types, scratch, root, variance, &mut |_, node, context| {
        if node.binds_quantifiers() {
            return Visit::Skip;
        }
        match *node {
            TypeNode::Quantified { index, .. } => at(index, context),
            _ => Visit::Descend,
        }
    })
}

fn visit_at<'run>(
    types: &TypeRegistry<'run>,
    scratch: BumpAllocator<'_>,
    kt: Handle,
    context: &mut Context,
    at: &mut impl FnMut(Handle, &TypeNode<'run>, &Context) -> Visit,
) -> bool {
    let node = types.node(kt);
    match at(kt, &node, context) {
        Visit::Stop => return true,
        Visit::Skip => return false,
        Visit::Descend => {}
    }
    let kids = child_list(scratch, &node);
    if kids.is_empty() {
        return false;
    }
    let binder = node.binds_quantifiers();
    if binder {
        context.binder_depth += 1;
    }
    let outer = context.variance;
    let mut stopped = false;
    for (child, flips) in kids.iter().copied() {
        context.variance = if flips { outer.flipped() } else { outer };
        if visit_at(types, scratch, child, context, at) {
            stopped = true;
            break;
        }
    }
    context.variance = outer;
    if binder {
        context.binder_depth -= 1;
    }
    stopped
}

/// Rebuild `root` with `rule` applied at every node before descent: `Some(k)` replaces the node and
/// stops there, `None` lets the driver descend.
///
/// Rebuilt composites re-intern through the registry's ordinary doors, so a rebuild that changes
/// nothing returns the input handle and interns not one node.
pub fn rebuild<'run>(
    types: &TypeRegistry<'run>,
    scratch: BumpAllocator<'_>,
    root: Handle,
    cfg: Rebuild,
    rule: &mut impl FnMut(Handle, &TypeNode<'run>, &Context) -> Option<Handle>,
) -> Handle {
    let mut context = Context::root(Variance::Co);
    rebuild_at(types, scratch, root, cfg, &mut context, rule)
}

fn rebuild_at<'run>(
    types: &TypeRegistry<'run>,
    scratch: BumpAllocator<'_>,
    kt: Handle,
    cfg: Rebuild,
    context: &mut Context,
    rule: &mut impl FnMut(Handle, &TypeNode<'run>, &Context) -> Option<Handle>,
) -> Handle {
    let node = types.node(kt);
    if let Some(replacement) = rule(kt, &node, context) {
        return replacement;
    }
    let kids = child_list(scratch, &node);
    if kids.is_empty() {
        return kt;
    }
    let binder = node.binds_quantifiers();
    if binder {
        context.binder_depth += 1;
    }
    let outer = context.variance;
    let mut rebuilt = BumpVec::with_capacity_in(kids.len(), scratch);
    let mut changed = false;
    for (child, flips) in kids.iter().copied() {
        context.variance = if flips { outer.flipped() } else { outer };
        let new = rebuild_at(types, scratch, child, cfg, context, rule);
        changed |= new != child;
        rebuilt.push(new);
    }
    context.variance = outer;
    if binder {
        context.binder_depth -= 1;
    }
    if !changed {
        return kt;
    }
    reassemble(types, scratch, &node, &rebuilt, cfg).unwrap_or(kt)
}

/// A node's children, in [`children`]'s order, in a buffer sized to their count.
fn child_list<'s>(scratch: BumpAllocator<'s>, node: &TypeNode<'_>) -> BumpVec<'s, (Handle, bool)> {
    let mut count = 0;
    children(node, &mut |_, _| count += 1);
    let mut kids = BumpVec::with_capacity_in(count, scratch);
    children(node, &mut |kt, flips| kids.push((kt, flips)));
    kids
}

/// **The arm table.** Every compound node's child handles, in the order [`reassemble`] reads them
/// back, each handed to `out` with whether its position flips variance.
///
/// A signature and a sealed member are leaves: one is closed content, the other content-addressed
/// by its component.
pub fn children(node: &TypeNode<'_>, out: &mut impl FnMut(Handle, bool)) {
    match *node {
        TypeNode::Number
        | TypeNode::Str
        | TypeNode::Bool
        | TypeNode::Null
        | TypeNode::Identifier
        | TypeNode::Symbol
        | TypeNode::TypeNameToken
        | TypeNode::Expression
        | TypeNode::SigiledTypeExpr
        | TypeNode::RecordType
        | TypeNode::Literal
        | TypeNode::Block
        | TypeNode::Declaration
        | TypeNode::Binder
        | TypeNode::Name
        | TypeNode::Keyword
        | TypeNode::Any
        | TypeNode::AnyValue
        | TypeNode::AnyCode
        // Its kind is a ground code leaf, so nothing under it is ever rebuilt.
        | TypeNode::CodeNeeding { .. }
        | TypeNode::Never
        | TypeNode::OfKind(_)
        | TypeNode::DeferredReturn(_)
        | TypeNode::Sibling(_)
        // A carrier's met bound is payload, read by a signature's fit alone.
        | TypeNode::Carrier { .. }
        | TypeNode::Signature { .. }
        | TypeNode::SetMember { .. } => {}
        TypeNode::List { element } => out(element, false),
        TypeNode::Dict { key, value } => {
            out(key, false);
            out(value, false);
        }
        TypeNode::Record { fields } => fields.values().for_each(|kt| out(kt, false)),
        TypeNode::KFunction { params, ret, .. } => {
            params.values().for_each(|kt| out(kt, true));
            out(ret, false);
        }
        TypeNode::ExpressionShape { elements, ret, .. } => {
            for element in elements.iter() {
                if let DispatchTokenElement::Slot(kt) = element {
                    out(*kt, true);
                }
            }
            out(ret, false);
        }
        TypeNode::Union { members } => members.iter().for_each(|m| out(*m, false)),
        // A bound is concrete, and a rebuild that reaches inside one keeps it concrete: no rule
        // here finds a variable in a bound to replace.
        TypeNode::ConstructorApply {
            constructor,
            arguments,
        } => {
            out(constructor, false);
            arguments.values().for_each(|kt| out(kt, false));
        }
        // A rigid variable's one child is its bound; a lexical variable's are its two ends.
        TypeNode::Lexical { lower, bound, .. } => {
            out(lower.raw(), false);
            out(bound.raw(), false);
        }
        TypeNode::Quantified { bound, .. } | TypeNode::Parameter { bound, .. } => {
            out(bound.raw(), false)
        }
        // An application's pins; its signature is closed.
        TypeNode::SignatureApply { pins, .. } => pins.values().for_each(|kt| out(kt, false)),
        TypeNode::SignatureMeet { members } => members.iter().for_each(|m| out(*m, false)),
    }
}

/// **The other half of the arm table.** One node plus replacement children, in [`children`]'s
/// order, re-interned through the registry's own doors. `None` for a node with no rebuild.
fn reassemble(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    node: &TypeNode<'_>,
    new: &[Handle],
    cfg: Rebuild,
) -> Option<Handle> {
    Some(match *node {
        TypeNode::List { .. } => types.list(new[0]),
        TypeNode::Dict { .. } => types.dict(new[0], new[1]),
        TypeNode::Record { fields } => {
            let mut values = new.iter();
            let fields = map_fields(scratch, fields.raw(), |_| {
                *values.next().expect("one replacement per field")
            });
            types.record(scratch, &fields)
        }
        TypeNode::KFunction {
            quantifiers,
            bounds,
            params,
            ..
        } => {
            let (values, ret) = new.split_at(params.len());
            let mut values = values.iter();
            let params = map_fields(scratch, params.raw(), |_| {
                *values.next().expect("one replacement per parameter")
            });
            types
                .function_group(scratch, quantifiers, bounds, &params, ret[0])
                .0
        }
        TypeNode::ExpressionShape {
            quantifiers,
            bounds,
            elements,
            classes,
            ..
        } => {
            let mut slots = new.iter();
            let rebuilt = map_slots(scratch, elements.raw(), |_| {
                *slots.next().expect("one replacement per slot position")
            });
            let ret = *slots.next().expect("the return follows the slots");
            types
                .shape_group(scratch, quantifiers, bounds, &rebuilt, classes, ret)
                .0
        }
        TypeNode::Union { .. } => match cfg.union {
            UnionDoor::Canonical => types.union_of(scratch, new),
            UnionDoor::Flat => types.intern_union_flat(scratch, new),
        },
        TypeNode::ConstructorApply { arguments, .. } => {
            let mut values = new[1..].iter();
            let arguments = map_fields(scratch, arguments.raw(), |_| {
                *values.next().expect("one replacement per argument")
            });
            types.constructor_apply(scratch, wrap(new[0]), &arguments)
        }
        TypeNode::Quantified { index, .. } => types.quantified(index, wrap(new[0])).raw(),
        TypeNode::Lexical { level, name, .. } => types
            .lexical_between(scratch, level, name, wrap(new[0]), wrap(new[1]))
            .raw(),
        TypeNode::Parameter { name, .. } => types.parameter(name, wrap(new[0])),
        TypeNode::SignatureApply { signature, pins } => {
            let mut values = new.iter();
            let pins = map_fields(scratch, pins.raw(), |_| {
                *values.next().expect("one replacement per pin")
            });
            types.signature_apply(scratch, signature, &pins)
        }
        TypeNode::SignatureMeet { .. } => types.signature_meet(scratch, new),
        _ => return None,
    })
}
