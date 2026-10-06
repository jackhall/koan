//! Layout order: where a member sits in a module, and the one place that rule is spelled.
//!
//! > Value members, sorted by name; then type members, sorted by name, a signature's parameters and
//! > manifest members merged into one run; then a body-born module's registrations, in slot order.
//!
//! Three things agree on it, and none of them consults the others. A body shape lays its value
//! slots out first, its type slots after and its registration slots last, each channel sorted
//! ([`scope`](crate::scope)'s three channels), so a body-born module's member run is its finished
//! activation's slots read out in slot order. A signature's member tables are symbol-sorted by
//! name, so a view's member run is built by walking them. A `USING` block's parameters are the
//! surfaced names, which the shape builder sorts the same way. So member `k` of a channel is slot
//! `k` of that channel everywhere, and `m.f` is an index, not a search. A signature names a
//! keyworded member by its shape, never by a slot, so the registration run is the tail past every
//! named member: a body-born module's registrations in slot order, or each overload a view carries
//! for its signature's keyworded members, in the signature's order. A reader finds a key's
//! functions by their registered shapes ([`functions_at`]), never by position.
//!
//! The sort is by interned symbol, which is a hash — not by the text of the name. Nothing reads
//! the order as alphabetical, and a test that pins one must read the symbols, not the source.

use crate::knot::{KValue, Knotted};
use crate::memory::{BumpAllocator, BumpVec};
use crate::symbols::{BinderSymbol, KeySymbol, TypeSymbol};
use crate::type_lattice::{
    DeclaredType, DispatchTokenElement, KType, Members, Parametric, SigSchema, TypeNode,
    TypeRegistry,
};

/// How many members of a module of signature `schema` are values — the index the type channel
/// starts at.
pub fn value_count(schema: &SigSchema<'_>) -> usize {
    schema.value_slots.len()
}

/// How many members a module of signature `schema` has.
pub fn member_count(schema: &SigSchema<'_>, scratch: BumpAllocator<'_>) -> usize {
    value_count(schema) + type_members(schema, scratch).len()
}

/// The type members in layout order: a declared signature's parameters and its manifest members
/// merged into one run by name. A module's schema has no parameters.
pub fn type_members<'x>(
    schema: &SigSchema<'_>,
    scratch: BumpAllocator<'x>,
) -> Members<'x, TypeSymbol> {
    Members::from_pairs(
        scratch,
        schema
            .parameters
            .iter()
            .chain(schema.manifest_members)
            .copied(),
    )
}

/// Where `name` sits in a module of signature `schema`, or `None` if the signature does not name
/// it.
pub fn member_index(
    schema: &SigSchema<'_>,
    scratch: BumpAllocator<'_>,
    name: BinderSymbol,
) -> Option<usize> {
    match name {
        BinderSymbol::Value(name) => rank(&schema.value_slots, name),
        BinderSymbol::Type(name) => {
            rank(&type_members(schema, scratch), name).map(|rank| value_count(schema) + rank)
        }
        // A signature names a keyworded member by its shape, never by a slot's name.
        BinderSymbol::Registration(_) | BinderSymbol::Key(_) => None,
    }
}

/// The registration members of `module` — the tail of its run past the named members, each a
/// function a keyworded use at its registered shape's key may select: one per registration of a
/// body-born module, and one per overload a view carries for a keyworded member of its signature.
/// Empty for anything that is no module.
pub fn registrations<'graph, 'cell>(
    module: Knotted<'graph, 'cell>,
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
) -> &'cell [KValue<'graph, 'cell>] {
    let Some((node, schema)) = module
        .module()
        .and_then(|node| Some((node, schema_of(node.ktype(), types)?)))
    else {
        return &[];
    };
    node.members()
        .get(member_count(&schema, scratch)..)
        .unwrap_or(&[])
}

/// Where `name` sits in a symbol-sorted member table — a binary search, since a table is built
/// only sorted.
fn rank<N: Ord + Copy, T>(table: &[(N, T)], name: N) -> Option<usize> {
    table.binary_search_by(|(held, _)| held.cmp(&name)).ok()
}

/// The schema of the signature `handle` is, if it is one.
pub fn schema_of<'run>(handle: KType, types: &TypeRegistry<'run>) -> Option<SigSchema<'run>> {
    match types.node(handle) {
        TypeNode::Signature { schema, .. } => Some(schema),
        _ => None,
    }
}

/// The member of `module` named `name` — what `m.f` reads. `None` if `module` is no module, or
/// its signature does not name `name`.
pub fn member<'graph, 'cell>(
    module: Knotted<'graph, 'cell>,
    name: BinderSymbol,
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
) -> Option<KValue<'graph, 'cell>> {
    let node = module.module()?;
    let schema = schema_of(node.ktype(), types)?;
    let index = member_index(&schema, scratch, name)?;
    node.members().get(index).copied()
}

/// The functions `module` offers at `key`: each registration member whose
/// registered shape ([`registered_shape`]) has that key.
pub fn functions_at<'graph, 'cell, 'x>(
    module: Knotted<'graph, 'cell>,
    key: KeySymbol,
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'x>,
) -> BumpVec<'x, KValue<'graph, 'cell>> {
    let mut found = BumpVec::new_in(scratch);
    for member in registrations(module, types, scratch) {
        let shape = registered_shape(*member).expect("a registration member is a function");
        if key_of(shape, types) == key {
            found.push(*member);
        }
    }
    found
}

/// The expression shape a registration member registers at: a function's own, or the shape a
/// barrier shows its caller. `None` for any other value.
pub fn registered_shape(member: KValue<'_, '_>) -> Option<DeclaredType<KType>> {
    let callable = member.as_callable()?;
    match (callable.function(), callable.coerced()) {
        (Some(function), _) => function.registered_shape(),
        (None, Some(barrier)) => Some(barrier.ktype()),
        (None, None) => None,
    }
}

/// The bucket key of the expression shape `shape`: its keywords in place, each slot erased. A
/// scheme's node spells its keys as a type's does.
pub fn key_of(shape: DeclaredType<impl Into<Parametric>>, types: &TypeRegistry<'_>) -> KeySymbol {
    let node = match shape {
        DeclaredType::Type(shape) => types.node(shape.into()),
        DeclaredType::Scheme(scheme) => types.scheme_node(scheme),
    };
    let TypeNode::ExpressionShape { elements, .. } = node else {
        unreachable!("a registered shape is an expression shape")
    };
    KeySymbol::of(elements.iter().map(|element| match element {
        DispatchTokenElement::Keyword(keyword) => Some(keyword),
        DispatchTokenElement::Slot(_) => None,
    }))
}
