//! Members born coerced: what an opaque view does to each thing it carries across its barrier.
//!
//! Under `:|` a view's abstract members are fresh mints, so a member declared at one of them no
//! longer has the type it had in the source. The member is therefore not carried but rebuilt at
//! the view's types: data is re-tagged through the admission barrier, a container is rebuilt cell
//! by cell and re-stamped, a nested module is re-viewed, and a function is wrapped in a
//! [barrier node](crate::function::coerced) a call will later go through.
//!
//! **The walk recurses on the declared type**, never on the two substituted types in lockstep. A
//! union interns its members in a canonical order, so the source's substitution and the view's do
//! not correspond position for position; only the declaration does. At each step the two
//! substitutions of the *declared* type say what the member is coming from and going to, and where
//! they agree there is nothing to do — which is the whole of `:!`, and every concrete slot of `:|`.
//!
//! Going the other way — a value arriving at a barrier from outside — is the call's work, and
//! [modules](../../roadmap/rewrite/modules.md)'.

use crate::function::{KValue, Knotted, coerced};
use crate::memory::{BumpAllocator, BumpVec, ScopeId, Writer};
use crate::symbols::TypeSymbol;
use crate::type_lattice::{
    KType, Members, SchemaDraft, TypeNode, TypeRegistry, satisfied_by, substitute_sig_members,
};
use crate::values::{Dict, List, Record, SealRefused, Tagged, Value};

use super::view;

/// Why a member could not take the view's type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoercionRefused {
    /// The admission barrier refused the mint or the witness.
    Seal(SealRefused),
    /// A slot declared a function type and the member is no function.
    NotAFunction,
    /// A slot declared a signature and the member is no module.
    NotAModule,
    /// A nested module could not take the view its slot declares.
    Nested,
    /// A union slot: no declared member's source side admits the value.
    NoUnionMember,
    /// A declared type this walk has no arm for, or a value whose shape does not match the arm
    /// its declaration took. A **cyclic data value** lands here: a container that is a knot's data
    /// node is a knot member, not a container word, and nothing yet rebuilds one through a
    /// barrier.
    Unsupported(KType),
}

/// What a coercion reads: the region it writes into, the lattice, and the two substitutions a
/// declared type is read under — the source module's bindings and the view's.
pub struct Coercion<'a, 'cell, 'run, 'x> {
    pub writer: Writer<'cell>,
    pub types: &'a TypeRegistry<'run>,
    pub scratch: BumpAllocator<'x>,
    /// What the source module binds the signature's abstract members to.
    pub from: Members<'x, TypeSymbol>,
    /// What the view binds them to: the mints under `:|`, the source's own under `:!`.
    pub to: Members<'x, TypeSymbol>,
}

impl<'cell, 'run, 'x> Coercion<'_, 'cell, 'run, 'x> {
    /// `declared` read under the source module's bindings.
    fn source_side(&self, declared: KType) -> KType {
        substitute_sig_members(
            self.types,
            self.scratch,
            declared,
            ScopeId::SENTINEL,
            self.from,
        )
    }

    /// `declared` read under the view's bindings.
    fn view_side(&self, declared: KType) -> KType {
        substitute_sig_members(
            self.types,
            self.scratch,
            declared,
            ScopeId::SENTINEL,
            self.to,
        )
    }

    /// A `Signature` handle whose manifest members are `table` — what a barrier node holds, since
    /// a node carries `Copy`, lifetime-free handles and not a borrowed table.
    fn sig_of(&self, table: Members<'_, TypeSymbol>) -> KType {
        let mut draft = SchemaDraft::new(self.scratch);
        for (name, kt) in table.iter().copied() {
            draft.insert_manifest(name, kt);
        }
        self.types.signature(self.scratch, draft)
    }
}

/// `value`, which the source module holds at `declared` read under `cx.from`, rebuilt at
/// `declared` read under `cx.to`.
pub fn coerce<'graph, 'cell>(
    cx: &Coercion<'_, 'cell, '_, '_>,
    value: KValue<'graph, 'cell>,
    declared: KType,
) -> Result<KValue<'graph, 'cell>, CoercionRefused> {
    let (src, dst) = (cx.source_side(declared), cx.view_side(declared));
    // The two sides agree, so the member already has the type the view declares: the whole of
    // `:!`, and every slot of `:|` that names no abstract member.
    if src == dst {
        return Ok(value);
    }
    match cx.types.node(declared) {
        // A reference to an abstract member, first-order or applied: the value takes the mint as
        // its one tagged layer, the barrier checking the mint is one and the payload fits.
        TypeNode::AbstractType { nonce: None, .. } | TypeNode::ConstructorApply { .. } => {
            Tagged::seal(cx.writer, value, dst, src, cx.types, cx.scratch)
                .map(Value::Tagged)
                .map_err(CoercionRefused::Seal)
        }
        TypeNode::List { element } => {
            let Value::List(list) = value else {
                return Err(CoercionRefused::Unsupported(declared));
            };
            let mut cells = BumpVec::with_capacity_in(list.len(), cx.scratch);
            for cell in list.cells() {
                cells.push(coerce(cx, *cell, element)?);
            }
            let built = List::new(cx.writer, cells.iter().copied(), cx.types, cx.scratch);
            Ok(Value::List(if built.ktype() == dst {
                built
            } else {
                built.with_type(cx.writer, dst)
            }))
        }
        // Keys are untouched: a dict's key type names no member a signature can declare abstract.
        TypeNode::Dict {
            value: cell_type, ..
        } => {
            let Value::Dict(dict) = value else {
                return Err(CoercionRefused::Unsupported(declared));
            };
            let mut entries = BumpVec::with_capacity_in(dict.len(), cx.scratch);
            for (key, cell) in dict.entries() {
                entries.push((*key, coerce(cx, *cell, cell_type)?));
            }
            let built = Dict::new(cx.writer, &entries, cx.types, cx.scratch);
            Ok(Value::Dict(if built.ktype() == dst {
                built
            } else {
                built.with_type(cx.writer, dst)
            }))
        }
        TypeNode::Record { fields } => {
            let Value::Record(record) = value else {
                return Err(CoercionRefused::Unsupported(declared));
            };
            let mut built = BumpVec::with_capacity_in(record.len(), cx.scratch);
            for (name, cell) in record.fields() {
                let (binder, field_type) = fields
                    .get_key_value(name)
                    .expect("the record satisfies the slot, so it has every declared field");
                built.push((binder, coerce(cx, *cell, field_type)?));
            }
            let built = Record::new(cx.writer, &built, cx.types, cx.scratch);
            Ok(Value::Record(if built.ktype() == dst {
                built
            } else {
                built.with_type(cx.writer, dst)
            }))
        }
        // The first declared member whose source side admits the value, in the union's interned
        // order. Two members that both admit it — a slot declared `Carrier | Number` over a source
        // binding `Carrier` to `Number` — take whichever that order reaches first.
        TypeNode::Union { .. } => {
            let carried = value.ktype();
            let member = union_members(cx, declared)
                .into_iter()
                .find(|member| satisfied_by(cx.types, cx.scratch, cx.source_side(*member), carried))
                .ok_or(CoercionRefused::NoUnionMember)?;
            coerce(cx, value, member)
        }
        TypeNode::KFunction { .. } => {
            let Value::Knotted(member) = value else {
                return Err(CoercionRefused::NotAFunction);
            };
            if member.function().is_none() && member.coerced().is_none() {
                return Err(CoercionRefused::NotAFunction);
            }
            let knot = coerced(
                cx.writer,
                member,
                dst,
                declared,
                cx.sig_of(cx.from),
                cx.sig_of(cx.to),
            );
            Ok(Value::Knotted(Knotted::of(knot, 0)))
        }
        // A nested module is re-viewed, and nothing is minted at the boundary: the nested
        // signature's own members were substituted when its slot was declared, so its slot types
        // name the *enclosing* signature's members and the enclosing substitutions read them. A
        // signature that names none of them never reaches here — its two sides agree and the
        // comparison above carried the module already.
        TypeNode::Signature { schema, .. } => {
            let Value::Knotted(member) = value else {
                return Err(CoercionRefused::NotAModule);
            };
            if member.module().is_none() {
                return Err(CoercionRefused::NotAModule);
            }
            view::build(
                cx.writer, member, schema, dst, cx.from, cx.to, cx.types, cx.scratch,
            )
            .map(Value::Knotted)
            .map_err(|_| CoercionRefused::Nested)
        }
        _ => Err(CoercionRefused::Unsupported(declared)),
    }
}

/// The declared members of the union `declared`, staged in scratch.
fn union_members<'x>(cx: &Coercion<'_, '_, '_, 'x>, declared: KType) -> BumpVec<'x, KType> {
    let mut members = BumpVec::new_in(cx.scratch);
    if let TypeNode::Union { members: run, .. } = cx.types.node(declared) {
        members.extend_from_slice(run);
    }
    members
}
