//! The unary driver: one arm table, two entry points.
//!
//! [`children`] lists a compound node's child handles in a fixed order, each with the variance its
//! position carries; [`reassemble`] puts a node back together from replacement children in that
//! same order. Every structural recursion over one type is [`visit`] or [`rebuild`] over that
//! pair, so a new compound variant is a compile error in exactly two matches here plus the
//! descent-knob sites.
//!
//! Both entry points take the scratch allocator the walk's transient buffers — each node's child
//! list, the rebuilt children, the context's shadow stack — are built in.

use crate::memory::{BumpAllocator, BumpVec};
use crate::parse::{BinderSymbol, TypeSymbol};

use super::Variance;
use crate::type_lattice::handle::KType;
use crate::type_lattice::node::{NodeSchema, TypeNode};
use crate::type_lattice::record::Record;
use crate::type_lattice::registry::TypeRegistry;
use crate::type_lattice::schema::{SigSchema, canonical_overloads};
use crate::type_lattice::shape::DispatchTokenElement;

/// Whether a knobbed arm is descended or treated as a leaf.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Through,
    Leaf,
}

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

/// [`visit`]'s descent knobs.
#[derive(Clone, Copy)]
pub struct Descent {
    pub signature: Step,
    pub set_member: Step,
}

/// The knobs a probe over quantifier structure takes: a nested signature and a sealed member are
/// both opaque, so the walk reaches only what the type spells inline.
pub const LEAF: Descent = Descent {
    signature: Step::Leaf,
    set_member: Step::Leaf,
};

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

/// [`rebuild`]'s knobs. A sealed member cannot be rebuilt, so there is no `set_member` knob.
#[derive(Clone, Copy)]
pub struct Rebuild {
    pub signature: Step,
    pub union: UnionDoor,
}

/// What the driver knows about the position a rule is looking at.
pub struct Context<'s> {
    /// Abstract member names declared by the descended signatures on the path here, innermost
    /// last. A name in the stack is bound by a nearer binder than any the rule is substituting for.
    shadow: BumpVec<'s, TypeSymbol>,
    shape_depth: usize,
    variance: Variance,
}

impl<'s> Context<'s> {
    fn root(scratch: BumpAllocator<'s>, variance: Variance) -> Self {
        Context {
            shadow: BumpVec::new_in(scratch),
            shape_depth: 0,
            variance,
        }
    }

    /// Whether an enclosing descended `Signature` declares an abstract member of this name.
    pub fn shadows(&self, name: TypeSymbol) -> bool {
        self.shadow.contains(&name)
    }

    /// Expression-shape binders on the path from the root to here, the root included: `0` means a
    /// [`TypeNode::Quantified`] seen here is **free**, `1` that it is bound by the root shape's own
    /// group.
    pub fn shape_depth(&self) -> usize {
        self.shape_depth
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
    root: KType,
    descent: Descent,
    at: &mut impl FnMut(KType, &TypeNode<'run>, &Context<'_>) -> Visit,
) -> bool {
    visit_in(types, scratch, root, descent, Variance::Co, at)
}

/// [`visit`] with the root's own polarity supplied — what a walk over a position already known to
/// be contravariant takes, such as the occurrence census over one argument slot of a shape being
/// interned, which has no shape node above it yet to flip the variance for it.
pub fn visit_in<'run>(
    types: &TypeRegistry<'run>,
    scratch: BumpAllocator<'_>,
    root: KType,
    descent: Descent,
    variance: Variance,
    at: &mut impl FnMut(KType, &TypeNode<'run>, &Context<'_>) -> Visit,
) -> bool {
    let mut context = Context::root(scratch, variance);
    visit_at(types, scratch, root, descent, &mut context, at)
}

fn visit_at<'run>(
    types: &TypeRegistry<'run>,
    scratch: BumpAllocator<'_>,
    kt: KType,
    descent: Descent,
    context: &mut Context<'_>,
    at: &mut impl FnMut(KType, &TypeNode<'run>, &Context<'_>) -> Visit,
) -> bool {
    let node = types.node(kt);
    match at(kt, &node, context) {
        Visit::Stop => return true,
        Visit::Skip => return false,
        Visit::Descend => {}
    }
    let kids = child_list(scratch, &node, descent.signature, descent.set_member);
    if kids.is_empty() {
        return false;
    }
    let depth = shadow_depth(&node, descent.signature, context);
    let binder = is_shape(&node);
    if binder {
        context.shape_depth += 1;
    }
    let outer = context.variance;
    let mut stopped = false;
    for (child, flips) in kids.iter().copied() {
        context.variance = if flips { outer.flipped() } else { outer };
        if visit_at(types, scratch, child, descent, context, at) {
            stopped = true;
            break;
        }
    }
    context.variance = outer;
    if binder {
        context.shape_depth -= 1;
    }
    context.shadow.truncate(depth);
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
    root: KType,
    cfg: Rebuild,
    rule: &mut impl FnMut(KType, &TypeNode<'run>, &Context<'_>) -> Option<KType>,
) -> KType {
    let mut context = Context::root(scratch, Variance::Co);
    rebuild_at(types, scratch, root, cfg, &mut context, rule)
}

fn rebuild_at<'run>(
    types: &TypeRegistry<'run>,
    scratch: BumpAllocator<'_>,
    kt: KType,
    cfg: Rebuild,
    context: &mut Context<'_>,
    rule: &mut impl FnMut(KType, &TypeNode<'run>, &Context<'_>) -> Option<KType>,
) -> KType {
    let node = types.node(kt);
    if let Some(replacement) = rule(kt, &node, context) {
        return replacement;
    }
    // A sealed member is content-addressed by its component, so it has no rebuild: the `Leaf`
    // knob here is the whole reason `Rebuild` carries no `set_member` field.
    let kids = child_list(scratch, &node, cfg.signature, Step::Leaf);
    if kids.is_empty() {
        return kt;
    }
    let depth = shadow_depth(&node, cfg.signature, context);
    let binder = is_shape(&node);
    if binder {
        context.shape_depth += 1;
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
        context.shape_depth -= 1;
    }
    context.shadow.truncate(depth);
    if !changed {
        return kt;
    }
    reassemble(types, scratch, &node, &rebuilt, cfg).unwrap_or(kt)
}

/// Push the abstract member names a descended signature declares, and hand back the stack depth to
/// truncate to on the way out.
fn shadow_depth(node: &TypeNode<'_>, signature: Step, context: &mut Context<'_>) -> usize {
    let depth = context.shadow.len();
    if signature == Step::Through
        && let TypeNode::Signature { schema, .. } = node
    {
        context
            .shadow
            .extend(schema.abstract_members.iter().map(|(name, _)| *name));
    }
    depth
}

fn is_shape(node: &TypeNode<'_>) -> bool {
    matches!(node, TypeNode::ExpressionShape { .. })
}

/// A node's children under the knobs, in [`children`]'s order, in a buffer sized to their count.
fn child_list<'s>(
    scratch: BumpAllocator<'s>,
    node: &TypeNode<'_>,
    signature: Step,
    set_member: Step,
) -> BumpVec<'s, (KType, bool)> {
    let mut count = 0;
    children(node, signature, set_member, &mut |_, _| count += 1);
    let mut kids = BumpVec::with_capacity_in(count, scratch);
    children(node, signature, set_member, &mut |kt, flips| {
        kids.push((kt, flips))
    });
    kids
}

/// **The arm table.** Every compound node's child handles, in the order [`reassemble`] reads them
/// back, each handed to `out` with whether its position flips variance.
///
/// A signature's members and a sealed member's schema arrive under their knobs; everything else is
/// unconditional. A signature's tables are read in their stored order — the one order the two
/// functions here agree on position for position.
pub fn children(
    node: &TypeNode<'_>,
    signature: Step,
    set_member: Step,
    out: &mut impl FnMut(KType, bool),
) {
    match *node {
        TypeNode::Number
        | TypeNode::Str
        | TypeNode::Bool
        | TypeNode::Null
        | TypeNode::Identifier
        | TypeNode::NameToken
        | TypeNode::TypeNameToken
        | TypeNode::KExpression
        | TypeNode::SigiledTypeExpr
        | TypeNode::RecordType
        | TypeNode::Any
        | TypeNode::Never
        | TypeNode::OfKind(_)
        | TypeNode::DeferredReturn(_)
        | TypeNode::Sibling(_) => {}
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
            for element in elements {
                if let DispatchTokenElement::Slot(kt) = element {
                    out(*kt, true);
                }
            }
            out(ret, false);
        }
        TypeNode::Union { members } => members.iter().for_each(|m| out(*m, false)),
        TypeNode::ConstructorApply {
            constructor,
            arguments,
        } => {
            out(constructor, false);
            arguments.values().for_each(|kt| out(kt, false));
        }
        // A rigid variable's one child is the bound it stands over.
        TypeNode::Quantified { bound, .. } | TypeNode::AbstractType { bound, .. } => {
            out(bound, false)
        }
        // Abstract members, then manifest members, then value slots, then the keyworded shapes.
        TypeNode::Signature { schema, .. } => {
            if signature == Step::Through {
                for (_, kt) in schema
                    .abstract_members
                    .iter()
                    .chain(schema.manifest_members)
                {
                    out(*kt, false);
                }
                for (_, kt) in schema.value_slots {
                    out(*kt, false);
                }
                for kt in schema.keyworded {
                    out(*kt, false);
                }
            }
        }
        TypeNode::SetMember { schema, .. } => {
            if set_member == Step::Through {
                match schema {
                    NodeSchema::NewType(repr) => out(repr, false),
                    NodeSchema::TypeConstructor { schema, .. } => {
                        schema.iter().for_each(|(_, kt)| out(*kt, false))
                    }
                }
            }
        }
    }
}

/// **The other half of the arm table.** One node plus replacement children, in [`children`]'s
/// order, re-interned through the registry's own doors. `None` for a node with no rebuild.
fn reassemble(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    node: &TypeNode<'_>,
    new: &[KType],
    cfg: Rebuild,
) -> Option<KType> {
    Some(match *node {
        TypeNode::List { .. } => types.list(new[0]),
        TypeNode::Dict { .. } => types.dict(new[0], new[1]),
        TypeNode::Record { fields } => types.record(scratch, &rekey(scratch, fields, new)),
        TypeNode::KFunction { params, .. } => {
            let (values, ret) = new.split_at(params.len());
            types.function_type(scratch, &rekey(scratch, params, values), ret[0])
        }
        TypeNode::ExpressionShape {
            quantifiers,
            elements,
            ..
        } => {
            let mut slots = new.iter();
            let mut rebuilt = BumpVec::with_capacity_in(elements.len(), scratch);
            rebuilt.extend(elements.iter().map(|element| match element {
                DispatchTokenElement::Slot(_) => DispatchTokenElement::Slot(
                    *slots.next().expect("one replacement per slot position"),
                ),
                keyword => *keyword,
            }));
            let ret = *slots.next().expect("the return follows the slots");
            types.shape_type(scratch, quantifiers, &rebuilt, ret).handle
        }
        TypeNode::Union { .. } => match cfg.union {
            UnionDoor::Canonical => types.union_of(scratch, new),
            UnionDoor::Flat => types.intern_union_flat(scratch, new),
        },
        TypeNode::ConstructorApply { arguments, .. } => {
            types.constructor_apply(scratch, new[0], &rekey(scratch, arguments, &new[1..]))
        }
        TypeNode::Quantified { index, .. } => types.quantified(index, new[0]),
        TypeNode::AbstractType {
            source,
            name,
            param_names,
            nonce,
            ..
        } => types.abstract_type(scratch, source, name, param_names, nonce, new[0]),
        TypeNode::Signature { schema, .. } => rebuilt_schema(types, scratch, schema, new),
        // A sealed member is keyed by its component's digest, which was computed over exactly the
        // schema it carries: rebuilding one would name content its handle contradicts.
        TypeNode::SetMember { .. } => return None,
        _ => return None,
    })
}

/// A record's keys in declaration order over replacement values in the same order.
fn rekey<'s>(
    scratch: BumpAllocator<'s>,
    record: Record<'_>,
    values: &[KType],
) -> BumpVec<'s, (BinderSymbol, KType)> {
    let mut fields = BumpVec::with_capacity_in(record.len(), scratch);
    fields.extend(record.keys().zip(values.iter().copied()));
    fields
}

/// `schema` with every member handle replaced by its rebuild, read back in [`children`]'s order and
/// interned. The named tables keep their names, so they stay sorted; the keyworded channel
/// re-canonicalizes, since two overloads may have become one.
fn rebuilt_schema(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    schema: SigSchema<'_>,
    new: &[KType],
) -> KType {
    let mut next = new.iter().copied();
    let mut replaced = || next.next().expect("one replacement per member");
    let abstract_members = scratch.slice_from_iter(
        schema
            .abstract_members
            .iter()
            .map(|(name, _)| (*name, replaced())),
    );
    let manifest_members = scratch.slice_from_iter(
        schema
            .manifest_members
            .iter()
            .map(|(name, _)| (*name, replaced())),
    );
    let value_slots = scratch.slice_from_iter(
        schema
            .value_slots
            .iter()
            .map(|(name, _)| (*name, replaced())),
    );
    let mut keyworded = BumpVec::with_capacity_in(schema.keyworded.len(), scratch);
    keyworded.extend(schema.keyworded.iter().map(|_| replaced()));
    canonical_overloads(types, scratch, &mut keyworded);
    types.intern_schema(SigSchema {
        sig_id: schema.sig_id,
        abstract_members,
        manifest_members,
        value_slots,
        keyworded: &keyworded,
        operators: schema.operators,
    })
}
