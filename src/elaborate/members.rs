//! A signature's members: where each sits in a module of the signature, and what the signature
//! declares it at — the one lookup the type reader ([`reads`](super::reads)), the builtin rules
//! and the module layout share.
//!
//! **Layout order.**
//!
//! > Value members, sorted by name; then type members, sorted by name, a signature's parameters and
//! > manifest members merged into one run; then a body-born module's registrations, in slot order.
//!
//! The signature alone places a named member ([`schema_member`]): a body-born module's tie and a
//! `USING` block place each member by name through it, so `m.f` is an index, not a search, and
//! nothing assumes a body's or a block's slot order matches. A signature names a keyworded member
//! by its shape, never by a slot, so the registration run is the tail past every named member: a
//! body-born module's registrations in slot order, or each overload a view carries for its
//! signature's keyworded members, in the signature's order.
//!
//! The sort is by interned symbol, which is a hash — not by the text of the name. Nothing reads
//! the order as alphabetical, and a test that pins one must read the symbols, not the source.

use crate::memory::{BumpAllocator, BumpVec};
use crate::symbols::{BinderSymbol, TypeSymbol};
use crate::type_lattice::{
    DeclaredType, KType, Members, Parametric, SigSchema, TypeNode, TypeRegistry, bound_above,
    substitute_parameters,
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

/// Where the member `name` sits in a module of signature `schema`, beside the type `schema`
/// declares it at: a value slot's type or scheme, or a type member's — a manifest member's type or
/// a head parameter. `None` where `schema` names no such member, and for a registration or a key,
/// which a signature names by shape.
pub fn schema_member(
    schema: &SigSchema<'_>,
    scratch: BumpAllocator<'_>,
    name: BinderSymbol,
) -> Option<(usize, DeclaredType<Parametric>)> {
    match name {
        BinderSymbol::Value(name) => {
            let index = rank(&schema.value_slots, name)?;
            Some((index, schema.value_slots[index].1))
        }
        BinderSymbol::Type(name) => {
            let members = type_members(schema, scratch);
            let index = rank(&members, name)?;
            Some((value_count(schema) + index, members[index].1.into()))
        }
        BinderSymbol::Registration(_) | BinderSymbol::Key(_) => None,
    }
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

/// A member a signature declares, as a read of it sees it.
#[derive(Clone, Copy)]
pub struct SignatureMember {
    /// Its declared type — a type member's own type, a value slot's type or scheme — each pin of
    /// the application substituted.
    pub declared: DeclaredType<Parametric>,
    /// The first head parameter it names that the application leaves unpinned, which stands for a
    /// different type at each module the read may reach.
    pub unpinned: Option<TypeSymbol>,
}

/// The member `name` of every module under `upper`, a signature or an application of one read
/// through a rigid variable's bound — a union's value member joined over its members: `None` where
/// `upper` is no such type, or declares no such member.
pub fn signature_member(
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
    upper: Parametric,
    name: BinderSymbol,
) -> Option<SignatureMember> {
    let upper = bound_above(types, scratch, upper);
    let (declared, pinned) = match types.node(upper) {
        TypeNode::Signature { .. } => (upper, None),
        TypeNode::SignatureApply { signature, pins } => (signature, Some(pins)),
        // A union's value member is the join of its members', where each declares it at a type.
        TypeNode::Union { members } if matches!(name, BinderSymbol::Value(_)) => {
            let mut joined = BumpVec::with_capacity_in(members.len(), scratch);
            for each in members.iter() {
                match signature_member(types, scratch, each.into(), name)? {
                    SignatureMember {
                        declared: DeclaredType::Type(declared),
                        unpinned: None,
                    } => joined.push(declared),
                    _ => return None,
                }
            }
            return Some(SignatureMember {
                declared: DeclaredType::Type(types.union_of(scratch, &joined)),
                unpinned: None,
            });
        }
        _ => return None,
    };
    let schema = schema_of(declared, types)?;
    let (_, read) = schema_member(&schema, scratch, name)?;
    let mut pins = BumpVec::new_in(scratch);
    let mut open = BumpVec::new_in(scratch);
    for (parameter, _) in schema.parameters.iter() {
        match pinned.and_then(|pins| pins.get(BinderSymbol::Type(*parameter).symbol())) {
            Some(pin) => pins.push((*parameter, pin)),
            None => open.push(*parameter),
        }
    }
    let read = substitute_parameters(types, scratch, read, Members::from_table(pins));
    let unpinned = open
        .into_iter()
        .find(|parameter| types.mentions_parameter(scratch, read, *parameter));
    Some(SignatureMember {
        declared: read,
        unpinned,
    })
}
