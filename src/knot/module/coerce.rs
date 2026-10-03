//! Members born coerced: what an opaque view does to each thing it carries across its barrier.
//!
//! Under `:|` a view's unpinned head parameters are fresh mints, so a member declared at one of
//! them has a type other than the one it has in the source. The member is therefore not carried but rebuilt at
//! the view's types: data is re-tagged through the admission barrier, a container is rebuilt part
//! by part from what its type shows — a record from the fields its slot declares, and no other —
//! and re-stamped, a nested module is re-viewed, and a function is wrapped in a
//! [barrier node](super::Coerced) a call will later go through.
//!
//! **The walk recurses on the declared type**, never on the two substituted types in lockstep. A
//! union interns its members in a canonical order, so the source's substitution and the view's do
//! not correspond position for position; only the declaration does. At each step the two
//! substitutions of the *declared* type say what the member is coming from and going to, and where
//! they agree there is nothing to do — which is the whole of `:!`, and every concrete slot of `:|`.
//!
//! Going the other way — a value arriving at a barrier from outside — is the call's work, and
//! [modules](../../../roadmap/rewrite/modules.md)'.

use crate::knot::{KValue, Knotted};
use crate::memory::{BumpAllocator, BumpVec, Writer};
use crate::symbols::{BinderSymbol, TypeSymbol};
use crate::type_lattice::{
    DeclaredType, KType, Members, Parametric, SchemaDraft, TypeNode, TypeRegistry,
    fits_application, satisfied_by, substitute_parameters,
};
use crate::values::{Dict, List, Record, SealRefused, Tagged, Value};

use super::{Coerced, view};

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
    Unsupported(DeclaredType<Parametric>),
}

/// What a coercion reads: the region it writes into, the lattice, and the two substitutions a
/// declared type is read under — the source module's bindings and the view's.
pub struct Coercion<'a, 'cell, 'run, 'x> {
    pub writer: Writer<'cell>,
    pub types: &'a TypeRegistry<'run>,
    pub scratch: BumpAllocator<'x>,
    /// What the source module binds the signature's head parameters to.
    pub from: Members<'x, TypeSymbol, KType>,
    /// What the view binds them to: the mints under `:|`, the source's own under `:!`.
    pub to: Members<'x, TypeSymbol, KType>,
}

/// Why a member type read under a module's or a view's bindings is concrete: it reads only its
/// signature's head parameters, and each is bound to a concrete type.
const BOUND: &str = "a member type reads only head parameters, each bound";

impl<'cell, 'run, 'x> Coercion<'_, 'cell, 'run, 'x> {
    /// `declared` read under `bindings`.
    fn read(
        &self,
        declared: DeclaredType<Parametric>,
        bindings: Members<'_, TypeSymbol, KType>,
    ) -> DeclaredType<KType> {
        match substitute_parameters(self.types, self.scratch, declared, bindings) {
            DeclaredType::Type(kt) => DeclaredType::Type(self.types.concrete(kt).expect(BOUND)),
            DeclaredType::Scheme(scheme) => DeclaredType::Scheme(scheme),
        }
    }

    /// `declared` read under the source module's bindings.
    fn source_side(&self, declared: DeclaredType<Parametric>) -> DeclaredType<KType> {
        self.read(declared, self.from)
    }

    /// `declared` read under the view's bindings.
    fn view_side(&self, declared: DeclaredType<Parametric>) -> DeclaredType<KType> {
        self.read(declared, self.to)
    }

    /// A `Signature` handle whose manifest members are `table` — what a barrier node holds, since
    /// a node carries `Copy`, lifetime-free handles and not a borrowed table.
    fn sig_of(&self, table: Members<'_, TypeSymbol, KType>) -> KType {
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
    declared: DeclaredType<Parametric>,
) -> Result<KValue<'graph, 'cell>, CoercionRefused> {
    let (src, dst) = (cx.source_side(declared), cx.view_side(declared));
    // The two sides agree, so the member already has the type the view declares: the whole of
    // `:!`, and every slot of `:|` that names no unpinned parameter.
    if src == dst {
        return Ok(value);
    }
    let unsupported = CoercionRefused::Unsupported(declared);
    let node = match declared {
        DeclaredType::Type(declared) => cx.types.node(declared),
        DeclaredType::Scheme(scheme) => cx.types.scheme_node(scheme),
    };
    // Only a function slot is a scheme, so every other arm's two sides are types.
    let (src_type, dst_type) = (src.as_type(), dst.as_type());
    match node {
        // A reference to a head parameter: the value takes the mint as its one tagged layer, the
        // barrier checking the mint is one and the payload fits.
        TypeNode::Parameter { nonce: None, .. } => {
            let (Some(src), Some(dst)) = (src_type, dst_type) else {
                return Err(unsupported);
            };
            Tagged::seal(cx.writer, value, dst, src, cx.types, cx.scratch)
                .map(Value::Tagged)
                .map_err(CoercionRefused::Seal)
        }
        TypeNode::List { element } => {
            let (Value::List(list), Some(dst)) = (value, dst_type) else {
                return Err(unsupported);
            };
            let surface = value.surface(cx.types, cx.scratch).expect("a list opens");
            let mut cells = BumpVec::with_capacity_in(list.len(), cx.scratch);
            for at in 0..surface.len() {
                let cell = surface.child(at, cx.types, cx.scratch).value();
                cells.push(coerce(cx, cell, element.into())?);
            }
            let built = List::new(cx.writer, cells.iter().copied(), cx.types, cx.scratch);
            Ok(Value::List(if built.ktype() == dst {
                built
            } else {
                built.with_type(cx.writer, dst)
            }))
        }
        // Keys are untouched: a dict's key type names no parameter a view mints.
        TypeNode::Dict {
            value: cell_type, ..
        } => {
            let (Value::Dict(dict), Some(dst)) = (value, dst_type) else {
                return Err(unsupported);
            };
            let surface = value.surface(cx.types, cx.scratch).expect("a dict opens");
            let mut entries = BumpVec::with_capacity_in(dict.len(), cx.scratch);
            for at in 0..surface.len() {
                let cell = surface.child(at, cx.types, cx.scratch).value();
                entries.push((*surface.key(at), coerce(cx, cell, cell_type.into())?));
            }
            let built = Dict::new(cx.writer, &entries, cx.types, cx.scratch);
            Ok(Value::Dict(if built.ktype() == dst {
                built
            } else {
                built.with_type(cx.writer, dst)
            }))
        }
        TypeNode::Record { fields } => {
            let (Value::Record(_), Some(dst)) = (value, dst_type) else {
                return Err(unsupported);
            };
            let mut built = BumpVec::with_capacity_in(fields.len(), cx.scratch);
            for (binder, field_type) in fields.iter() {
                let cell = value
                    .field(binder.symbol(), cx.types, cx.scratch)
                    .expect("the record satisfies the slot, so it has every declared field")
                    .value();
                built.push((binder, coerce(cx, cell, field_type.into())?));
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
        TypeNode::Union { members } => {
            let carried = value.ktype();
            let member = members
                .iter()
                .find(|member| {
                    let source = cx.source_side((*member).into());
                    satisfied_by(cx.types, cx.scratch, source, carried)
                })
                .ok_or(CoercionRefused::NoUnionMember)?;
            coerce(cx, value, member.into())
        }
        TypeNode::KFunction { .. } => {
            let Value::Knotted(member) = value else {
                return Err(CoercionRefused::NotAFunction);
            };
            if member.function().is_none() && member.coerced().is_none() {
                return Err(CoercionRefused::NotAFunction);
            }
            let knot = Coerced::tie(
                cx.writer,
                member,
                dst,
                declared,
                cx.sig_of(cx.from),
                cx.sig_of(cx.to),
            );
            Ok(Value::Knotted(Knotted::of(knot, 0)))
        }
        // A nested module is re-viewed, and nothing is minted at the boundary: the application's
        // pins name the enclosing signature's parameters, which the enclosing substitutions read.
        // The nested signature's own unpinned parameters keep what *fits* solves them to either
        // side. An application naming none of the enclosing parameters never reaches here — its
        // two sides agree and the comparison above carried the module already.
        TypeNode::SignatureApply { signature, .. } => {
            let Value::Knotted(member) = value else {
                return Err(CoercionRefused::NotAModule);
            };
            let Some(module) = member.module() else {
                return Err(CoercionRefused::NotAModule);
            };
            let (Some(src), Some(dst)) = (src_type, dst_type) else {
                return Err(unsupported);
            };
            let (
                TypeNode::SignatureApply {
                    pins: from_pins, ..
                },
                TypeNode::SignatureApply { pins: to_pins, .. },
            ) = (cx.types.node(src), cx.types.node(dst))
            else {
                return Err(unsupported);
            };
            let Some(schema) = super::layout::schema_of(signature, cx.types) else {
                return Err(unsupported);
            };
            let mut pins = BumpVec::with_capacity_in(from_pins.len(), cx.scratch);
            pins.extend(from_pins.iter());
            let from = fits_application(cx.types, cx.scratch, module.ktype(), signature, &pins)
                .map_err(|_| CoercionRefused::Nested)?;
            let from = view::solved(cx.types, cx.scratch, from);
            let to = Members::from_pairs(
                cx.scratch,
                from.iter().map(|(name, solved)| {
                    let pinned = to_pins.get(BinderSymbol::Type(*name).symbol());
                    (*name, pinned.unwrap_or(*solved))
                }),
            );
            let view = view::view_signature(&schema, to, cx.types, cx.scratch);
            view::build(
                cx.writer, member, schema, view, from, to, cx.types, cx.scratch,
            )
            .map(Value::Knotted)
            .map_err(|_| CoercionRefused::Nested)
        }
        _ => Err(unsupported),
    }
}
