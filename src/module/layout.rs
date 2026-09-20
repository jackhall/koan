//! Layout order: where a member sits in a module, and the one place that rule is spelled.
//!
//! > Value members, sorted by name; then type members, sorted by name, abstract and manifest
//! > merged into one run.
//!
//! Three things agree on it, and none of them consults the others. A body shape lays its value
//! slots out first and its type slots after, each channel sorted
//! ([`scope`](crate::scope)'s two channels), so a body-born module's member run is its finished
//! activation's slots read out in slot order. A signature's member tables are symbol-sorted by
//! name, so a view's member run is built by walking them. A `USING` block's parameters are the
//! surfaced names, which the shape builder sorts the same way. So member `k` of a channel is slot
//! `k` of that channel everywhere, and `m.f` is an index, not a search.
//!
//! The sort is by interned symbol, which is a hash — not by the text of the name. Nothing reads
//! the order as alphabetical, and a test that pins one must read the symbols, not the source.

use crate::function::{KValue, Knotted};
use crate::memory::BumpAllocator;
use crate::parse::{BinderSymbol, TypeSymbol};
use crate::type_lattice::{KType, Members, SigSchema, TypeNode, TypeRegistry};

/// How many members of a module of signature `schema` are values — the index the type channel
/// starts at.
pub fn value_count(schema: &SigSchema<'_>) -> usize {
    schema.value_slots.len()
}

/// How many members a module of signature `schema` has.
pub fn member_count(schema: &SigSchema<'_>, scratch: BumpAllocator<'_>) -> usize {
    value_count(schema) + type_members(schema, scratch).len()
}

/// The type members in layout order: abstract and manifest merged into one run by name, manifest
/// winning where a name is both — the reading every relation over two schemas takes.
pub fn type_members<'x>(
    schema: &SigSchema<'_>,
    scratch: BumpAllocator<'x>,
) -> Members<'x, TypeSymbol> {
    schema.member_bindings(scratch)
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
    }
}

/// Where `name` sits in a symbol-sorted member table — a binary search, since a table is built
/// only sorted.
fn rank<N: Ord + Copy>(table: &[(N, KType)], name: N) -> Option<usize> {
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
