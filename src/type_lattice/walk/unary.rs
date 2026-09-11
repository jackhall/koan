//! The unary driver: one arm table, two entry points.
//!
//! [`children`] lists a compound node's child handles in a fixed order, each with the variance its
//! position carries; [`reassemble`] puts a node back together from replacement children in that
//! same order. Every structural recursion over one type is [`visit`] or [`rebuild`] over that
//! pair, so a new compound variant is a compile error in exactly two matches here plus the
//! descent-knob sites.

use smallvec::SmallVec;

use crate::parse::TypeSymbol;

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
pub struct Context {
    /// Abstract member names declared by the descended signatures on the path here, innermost
    /// last. A name in the stack is bound by a nearer binder than any the rule is substituting for.
    shadow: Vec<TypeSymbol>,
    shape_depth: usize,
    variance: Variance,
}

impl Context {
    fn root() -> Self {
        Context {
            shadow: Vec::new(),
            shape_depth: 0,
            variance: Variance::Co,
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

/// One child position: the handle, and whether the position flips variance.
type ChildList = SmallVec<[(KType, bool); 8]>;

/// Walk `root` pre-order, handing every node reached to `at`. Returns whether the walk stopped
/// early — the verdict a "does this type contain …" probe reads.
pub fn visit(
    types: &TypeRegistry,
    root: KType,
    descent: Descent,
    at: &mut impl FnMut(KType, &TypeNode, &Context) -> Visit,
) -> bool {
    visit_in(types, root, descent, Variance::Co, at)
}

/// [`visit`] with the root's own polarity supplied — what a walk over a position already known to
/// be contravariant takes, such as the occurrence census over one argument slot of a shape being
/// interned, which has no shape node above it yet to flip the variance for it.
pub fn visit_in(
    types: &TypeRegistry,
    root: KType,
    descent: Descent,
    variance: Variance,
    at: &mut impl FnMut(KType, &TypeNode, &Context) -> Visit,
) -> bool {
    let mut context = Context::root();
    context.variance = variance;
    visit_at(types, root, descent, &mut context, at)
}

fn visit_at(
    types: &TypeRegistry,
    kt: KType,
    descent: Descent,
    context: &mut Context,
    at: &mut impl FnMut(KType, &TypeNode, &Context) -> Visit,
) -> bool {
    types.with_node(kt, |node| {
        match at(kt, node, context) {
            Visit::Stop => return true,
            Visit::Skip => return false,
            Visit::Descend => {}
        }
        let mut kids = ChildList::new();
        children(node, descent.signature, descent.set_member, &mut kids);
        if kids.is_empty() {
            return false;
        }
        let depth = shadow_depth(node, descent.signature, context);
        let binder = is_shape(node);
        if binder {
            context.shape_depth += 1;
        }
        let outer = context.variance;
        let mut stopped = false;
        for (child, flips) in kids {
            context.variance = if flips { outer.flipped() } else { outer };
            if visit_at(types, child, descent, context, at) {
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
    })
}

/// Rebuild `root` with `rule` applied at every node before descent: `Some(k)` replaces the node and
/// stops there, `None` lets the driver descend.
///
/// Rebuilt composites re-intern through the registry's ordinary doors, so a rebuild that changes
/// nothing returns the input handle and interns not one node.
pub fn rebuild(
    types: &TypeRegistry,
    root: KType,
    cfg: Rebuild,
    rule: &mut impl FnMut(KType, &TypeNode, &Context) -> Option<KType>,
) -> KType {
    let mut context = Context::root();
    rebuild_at(types, root, cfg, &mut context, rule)
}

fn rebuild_at(
    types: &TypeRegistry,
    kt: KType,
    cfg: Rebuild,
    context: &mut Context,
    rule: &mut impl FnMut(KType, &TypeNode, &Context) -> Option<KType>,
) -> KType {
    types.with_node(kt, |node| {
        if let Some(replacement) = rule(kt, node, context) {
            return replacement;
        }
        let mut kids = ChildList::new();
        // A sealed member is content-addressed by its component, so it has no rebuild: the
        // `Leaf` knob here is the whole reason `Rebuild` carries no `set_member` field.
        children(node, cfg.signature, Step::Leaf, &mut kids);
        if kids.is_empty() {
            return kt;
        }
        let depth = shadow_depth(node, cfg.signature, context);
        let binder = is_shape(node);
        if binder {
            context.shape_depth += 1;
        }
        let outer = context.variance;
        let mut rebuilt: SmallVec<[KType; 8]> = SmallVec::with_capacity(kids.len());
        let mut changed = false;
        for (child, flips) in kids {
            context.variance = if flips { outer.flipped() } else { outer };
            let new = rebuild_at(types, child, cfg, context, rule);
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
        reassemble(types, node, &rebuilt, cfg).unwrap_or(kt)
    })
}

/// Push the abstract member names a descended signature declares, and hand back the stack depth to
/// truncate to on the way out.
fn shadow_depth(node: &TypeNode, signature: Step, context: &mut Context) -> usize {
    let depth = context.shadow.len();
    if signature == Step::Through
        && let TypeNode::Signature { schema, .. } = node
    {
        context
            .shadow
            .extend(schema.abstract_members.keys().copied());
    }
    depth
}

fn is_shape(node: &TypeNode) -> bool {
    matches!(node, TypeNode::ExpressionShape { .. })
}

/// **The arm table.** Every compound node's child handles, in the order [`reassemble`] reads them
/// back, each paired with whether its position flips variance.
///
/// A signature's members and a sealed member's schema arrive under their knobs; everything else is
/// unconditional. The two maps a signature keys by name feed in symbol order and its value slots
/// likewise, because a `HashMap`'s iteration order is not stable and the two functions here must
/// agree position for position.
fn children(node: &TypeNode, signature: Step, set_member: Step, out: &mut ChildList) {
    match node {
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
        TypeNode::List { element } => out.push((*element, false)),
        TypeNode::Dict { key, value } => {
            out.push((*key, false));
            out.push((*value, false));
        }
        TypeNode::Record { fields } => out.extend(fields.values().map(|v| (*v, false))),
        TypeNode::KFunction { params, ret } => {
            out.extend(params.values().map(|v| (*v, true)));
            out.push((*ret, false));
        }
        TypeNode::ExpressionShape { elements, ret, .. } => {
            out.extend(elements.iter().filter_map(|element| match element {
                DispatchTokenElement::Slot(kt) => Some((*kt, true)),
                DispatchTokenElement::Keyword(_) => None,
            }));
            out.push((*ret, false));
        }
        TypeNode::Union { members } => out.extend(members.iter().map(|m| (*m, false))),
        TypeNode::ConstructorApply {
            constructor,
            arguments,
        } => {
            out.push((*constructor, false));
            out.extend(arguments.values().map(|a| (*a, false)));
        }
        // A rigid variable's one child is the bound it stands over.
        TypeNode::Quantified { bound, .. } | TypeNode::AbstractType { bound, .. } => {
            out.push((*bound, false))
        }
        TypeNode::Signature { schema, .. } => {
            if signature == Step::Through {
                out.extend(signature_children(schema).map(|kt| (kt, false)));
            }
        }
        TypeNode::SetMember { schema, .. } => {
            if set_member == Step::Through {
                match schema {
                    NodeSchema::NewType(repr) => out.push((*repr, false)),
                    NodeSchema::TypeConstructor { schema, .. } => {
                        out.extend(sorted_members(schema).map(|(_, kt)| (kt, false)))
                    }
                }
            }
        }
    }
}

/// **The other half of the arm table.** One node plus replacement children, in [`children`]'s
/// order, re-interned through the registry's own doors. `None` for a node with no rebuild.
fn reassemble(types: &TypeRegistry, node: &TypeNode, new: &[KType], cfg: Rebuild) -> Option<KType> {
    let union_door = |members: &[KType]| match cfg.union {
        UnionDoor::Canonical => types.union_of(members),
        UnionDoor::Flat => types.intern_union_flat(members),
    };
    Some(match node {
        TypeNode::List { .. } => types.list(new[0]),
        TypeNode::Dict { .. } => types.dict(new[0], new[1]),
        TypeNode::Record { fields } => types.record(rekey(fields, new)),
        TypeNode::KFunction { params, .. } => {
            let (values, ret) = new.split_at(params.len());
            types.function_type(rekey(params, values), ret[0])
        }
        TypeNode::ExpressionShape {
            quantifiers,
            elements,
            ..
        } => {
            let mut slots = new.iter();
            let elements: SmallVec<[DispatchTokenElement; 12]> = elements
                .iter()
                .map(|element| match element {
                    DispatchTokenElement::Slot(_) => DispatchTokenElement::Slot(
                        *slots.next().expect("one replacement per slot position"),
                    ),
                    keyword => *keyword,
                })
                .collect();
            let ret = *slots.next().expect("the return follows the slots");
            types.shape_type(quantifiers, &elements, ret).handle
        }
        TypeNode::Union { .. } => union_door(new),
        TypeNode::ConstructorApply { arguments, .. } => {
            types.constructor_apply(new[0], rekey(arguments, &new[1..]))
        }
        TypeNode::Quantified { index, .. } => types.quantified(*index, new[0]),
        TypeNode::AbstractType {
            source,
            name,
            param_names,
            nonce,
            ..
        } => types.abstract_type(*source, *name, param_names.clone(), *nonce, new[0]),
        TypeNode::Signature { schema, .. } => types.signature(rebuilt_schema(types, schema, new)),
        // A sealed member is keyed by its component's digest, which was computed over exactly the
        // schema it carries: rebuilding one would name content its handle contradicts.
        TypeNode::SetMember { .. } => return None,
        _ => return None,
    })
}

/// A record's keys in declaration order over replacement values in the same order.
fn rekey(record: &Record<KType>, values: &[KType]) -> Record<KType> {
    record
        .keys()
        .zip(values.iter().copied())
        .collect::<Record<KType>>()
}

/// A signature's member handles in the one order [`children`] and [`rebuilt_schema`] agree on:
/// abstract members, then manifest members, then value slots, each in symbol order, then the
/// keyworded shapes in the schema's own canonical order.
fn signature_children(schema: &SigSchema) -> impl Iterator<Item = KType> + '_ {
    sorted_members(&schema.abstract_members)
        .map(|(_, kt)| kt)
        .chain(sorted_members(&schema.manifest_members).map(|(_, kt)| kt))
        .chain(sorted_slots(schema).map(|(_, kt)| kt))
        .chain(schema.keyworded.iter().copied())
        .collect::<Vec<_>>()
        .into_iter()
}

fn sorted_members(
    members: &crate::type_lattice::schema::TypeMemberMap,
) -> impl Iterator<Item = (TypeSymbol, KType)> {
    let mut pairs: Vec<(TypeSymbol, KType)> = members.iter().map(|(n, kt)| (*n, *kt)).collect();
    pairs.sort_unstable_by_key(|(name, _)| *name);
    pairs.into_iter()
}

fn sorted_slots(
    schema: &SigSchema,
) -> impl Iterator<Item = (crate::parse::ValueSymbol, KType)> + use<> {
    let mut pairs: Vec<(crate::parse::ValueSymbol, KType)> =
        schema.value_slots.iter().map(|(n, kt)| (*n, *kt)).collect();
    pairs.sort_unstable_by_key(|(name, _)| *name);
    pairs.into_iter()
}

/// `schema` with every member handle replaced by its rebuild, read back in [`signature_children`]'s
/// order. The keyworded channel re-canonicalizes, since two overloads may have become one.
fn rebuilt_schema(types: &TypeRegistry, schema: &SigSchema, new: &[KType]) -> SigSchema {
    let mut rebuilt = schema.clone();
    let mut next = new.iter().copied();
    for (name, _) in sorted_members(&schema.abstract_members) {
        rebuilt
            .abstract_members
            .insert(name, next.next().expect("one replacement per member"));
    }
    for (name, _) in sorted_members(&schema.manifest_members) {
        rebuilt
            .manifest_members
            .insert(name, next.next().expect("one replacement per member"));
    }
    for (name, _) in sorted_slots(schema) {
        rebuilt
            .value_slots
            .insert(name, next.next().expect("one replacement per slot"));
    }
    let keyworded: Vec<KType> = schema
        .keyworded
        .iter()
        .map(|_| next.next().expect("one replacement per keyworded member"))
        .collect();
    rebuilt.keyworded = canonical_overloads(keyworded, types);
    rebuilt
}
