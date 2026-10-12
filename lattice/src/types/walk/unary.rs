//! The unary driver: one arm table, three entry points.
//!
//! [`children`] lists a compound node's children in a fixed order, each with the variance its
//! position carries; [`reassemble`] puts a node back together from replacement children in that
//! same order. Every structural recursion over one type is [`visit`] or [`rebuild`] over that
//! pair, so a new compound variant is a compile error in exactly two matches here. Both walk nodes
//! by reference, from node to child: [`visit`] needs no chain at all, and [`rebuild`] takes one
//! only to re-intern what it rebuilds.
//! [`visit_free_quantified`] is [`visit`] narrowed to the free `Quantified` positions, the probe
//! every scheme check shares.
//!
//! A signature's members and a sealed member's schema are closed content: neither walk reaches
//! inside one.
//!
//! Both entry points take the scratch allocator the walk's transient buffers — each node's child
//! list and the rebuilt children — are built in.

use crate::bump::{BumpAllocator, BumpVec};

use super::Variance;
use crate::types::chain::TypeChain;
use crate::types::node::TypeNode;
use crate::types::record::map_fields;
use crate::types::shape::{DispatchTokenElement, map_slots};
use crate::types::stored::Linked;
use crate::types::tiers::Habitat;

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

    /// Schemes on the path from the root to here, the root included: `0` means a
    /// [`TypeNode::Quantified`] seen here is **free**, `1` that it is bound by the root scheme's own
    /// group.
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
    scratch: BumpAllocator<'_>,
    root: Linked<'run>,
    at: &mut impl FnMut(Linked<'run>, &TypeNode<'run, Linked<'run>>, &Context) -> Visit,
) -> bool {
    visit_in(scratch, root, Variance::Co, at)
}

/// [`visit`] with the root's own polarity supplied — what a walk over a position already known to
/// be contravariant takes, such as the occurrence census over one argument slot of a shape being
/// interned, which has no shape node above it yet to flip the variance for it.
pub fn visit_in<'run>(
    scratch: BumpAllocator<'_>,
    root: Linked<'run>,
    variance: Variance,
    at: &mut impl FnMut(Linked<'run>, &TypeNode<'run, Linked<'run>>, &Context) -> Visit,
) -> bool {
    let mut context = Context::root(variance);
    visit_at(scratch, root, &mut context, at)
}

/// [`visit_in`] over the **free** `Quantified` positions of `root` alone: a nested scheme is
/// skipped whole, every other node is descended, and each free variable's index is handed to
/// `at` with its context. Returns whether `at` stopped the walk.
pub fn visit_free_quantified(
    scratch: BumpAllocator<'_>,
    root: Linked<'_>,
    variance: Variance,
    at: &mut impl FnMut(usize, &Context) -> Visit,
) -> bool {
    visit_in(scratch, root, variance, &mut |_, node, context| {
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
    scratch: BumpAllocator<'_>,
    kt: Linked<'run>,
    context: &mut Context,
    at: &mut impl FnMut(Linked<'run>, &TypeNode<'run, Linked<'run>>, &Context) -> Visit,
) -> bool {
    let node = kt.view();
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
        if visit_at(scratch, child, context, at) {
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
/// Rebuilt composites re-intern through the chain's ordinary doors — a union through the
/// canonicalizing one — so a rebuild that changes nothing returns the input node and interns not
/// one node.
pub fn rebuild<'run>(
    types: &TypeChain<'run, impl Habitat<'run>>,
    scratch: BumpAllocator<'_>,
    root: Linked<'run>,
    rule: &mut impl FnMut(Linked<'run>, &TypeNode<'run, Linked<'run>>, &Context) -> Option<Linked<'run>>,
) -> Linked<'run> {
    let mut context = Context::root(Variance::Co);
    rebuild_at(types, scratch, root, &mut context, rule)
}

fn rebuild_at<'run>(
    types: &TypeChain<'run, impl Habitat<'run>>,
    scratch: BumpAllocator<'_>,
    kt: Linked<'run>,
    context: &mut Context,
    rule: &mut impl FnMut(Linked<'run>, &TypeNode<'run, Linked<'run>>, &Context) -> Option<Linked<'run>>,
) -> Linked<'run> {
    let node = kt.view();
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
        let new = rebuild_at(types, scratch, child, context, rule);
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
    reassemble(types, scratch, &node, &rebuilt).unwrap_or(kt)
}

/// A node's children, in [`children`]'s order, in a buffer sized to their count.
fn child_list<'s, 'run>(
    scratch: BumpAllocator<'s>,
    node: &TypeNode<'run, Linked<'run>>,
) -> BumpVec<'s, (Linked<'run>, bool)> {
    let mut count = 0;
    children(node, &mut |_, _| count += 1);
    let mut kids = BumpVec::with_capacity_in(count, scratch);
    children(node, &mut |kt, flips| kids.push((kt, flips)));
    kids
}

/// **The arm table.** Every compound node's children, in the order [`reassemble`] reads them back,
/// each handed to `out` with whether its position flips variance.
///
/// A signature and a sealed member are leaves: one is closed content, the other content-addressed
/// by its component.
pub fn children<'run>(
    node: &TypeNode<'run, Linked<'run>>,
    out: &mut impl FnMut(Linked<'run>, bool),
) {
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
        TypeNode::KFunction { params, ret } => {
            params.values().for_each(|kt| out(kt, true));
            out(ret, false);
        }
        TypeNode::ExpressionShape { elements, ret, .. } => {
            for element in elements.iter() {
                if let DispatchTokenElement::Slot(kt) = element {
                    out(kt, true);
                }
            }
            out(ret, false);
        }
        // The body's own node flips its parameters and slots.
        TypeNode::Scheme { body, .. } => out(body, false),
        TypeNode::Union { members } => members.iter().for_each(|m| out(m, false)),
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
            out(lower, false);
            out(bound, false);
        }
        TypeNode::Quantified { bound, .. } | TypeNode::Parameter { bound, .. } => out(bound, false),
        // An application's pins; its signature is closed.
        TypeNode::SignatureApply { pins, .. } => pins.values().for_each(|kt| out(kt, false)),
        TypeNode::SignatureMeet { members } => members.iter().for_each(|m| out(m, false)),
    }
}

/// **The other half of the arm table.** One node plus replacement children, in [`children`]'s
/// order, re-interned through the chain's own doors. `None` for a node with no children.
fn reassemble<'run>(
    types: &TypeChain<'run, impl Habitat<'run>>,
    scratch: BumpAllocator<'_>,
    node: &TypeNode<'run, Linked<'run>>,
    new: &[Linked<'run>],
) -> Option<Linked<'run>> {
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
        TypeNode::KFunction { params, .. } => {
            let (values, ret) = new.split_at(params.len());
            let mut values = values.iter();
            let params = map_fields(scratch, params.raw(), |_| {
                *values.next().expect("one replacement per parameter")
            });
            types.function_type(scratch, &params, ret[0])
        }
        TypeNode::ExpressionShape {
            elements, classes, ..
        } => {
            let mut slots = new.iter();
            let rebuilt = map_slots(scratch, elements.iter(), |_| {
                *slots.next().expect("one replacement per slot position")
            });
            let ret = *slots.next().expect("the return follows the slots");
            types.shape_type(&rebuilt, classes.copied_into(scratch), ret)
        }
        // The rebuilt body still reads the group in its old order, which the door renumbers: a
        // rebuild that drops a variable's first occurrence moves it.
        TypeNode::Scheme { group, .. } => {
            types
                .bind_group(scratch, group.names, group.bounds, new[0])
                .0
        }
        TypeNode::Union { .. } => types.union_of(scratch, new),
        TypeNode::ConstructorApply { arguments, .. } => {
            let mut values = new[1..].iter();
            let arguments = map_fields(scratch, arguments.raw(), |_| {
                *values.next().expect("one replacement per argument")
            });
            types.constructor_apply(scratch, new[0], &arguments)
        }
        TypeNode::Quantified { index, .. } => types.quantified_node(index, new[0]),
        TypeNode::Lexical { level, name, .. } => {
            types.lexical_node(scratch, level, name, new[0], new[1])
        }
        TypeNode::Parameter { name, .. } => types.parameter(name, new[0]),
        TypeNode::SignatureApply { signature, pins } => {
            let mut values = new.iter();
            let pins = map_fields(scratch, pins.raw(), |_| {
                *values.next().expect("one replacement per pin")
            });
            types.signature_apply(scratch, signature, &pins)
        }
        TypeNode::SignatureMeet { .. } => types.signature_meet(scratch, new),
        // No wildcard: [`children`] gives each of these no child, so none is ever rebuilt, and a
        // new compound variant fails to compile here rather than silently keeping its old handle.
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
        | TypeNode::CodeNeeding { .. }
        | TypeNode::Never
        | TypeNode::OfKind(_)
        | TypeNode::DeferredReturn(_)
        | TypeNode::Sibling(_)
        | TypeNode::Carrier { .. }
        | TypeNode::Signature { .. }
        | TypeNode::SetMember { .. } => return None,
    })
}
