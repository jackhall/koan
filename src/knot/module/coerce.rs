//! Members born coerced: what an opaque view does to each thing it carries across its barrier.
//!
//! Under `:|` a view's unpinned head parameters are carriers, so a member declared at one of
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
//! A call through a barrier crosses it both ways: its arguments [`inward`] — the walk with the two
//! substitutions swapped, where a carrier's side unseals a value sealed under it — and its result
//! [`outward`].

use crate::knot::{KValue, Knotted};
use crate::memory::{BumpAllocator, BumpVec, Writer};
use crate::symbols::{BinderSymbol, TypeSymbol};
use std::fmt;

use crate::scope::{IMPLICIT, ParameterBinding};
use crate::symbols::SymbolInterner;
use crate::type_lattice::{
    DeclaredType, DispatchTokenElement, KType, Members, Parametric, Record as TypeRecord,
    SchemaDraft, TypeNode, TypeRegistry, display_name, fits_application, satisfied_by,
    substitute_parameters,
};
use crate::values::{Dict, List, Record, SealRefused, Tagged, Value, satisfies};

use super::{Coerced, ModuleContent, Operator, view};

/// Why a member could not take the view's type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoercionRefused {
    /// The admission barrier refused the carrier or the witness.
    Seal(SealRefused),
    /// A slot declared a function type and the member is no function.
    NotAFunction,
    /// A slot declared a signature and the member is no module.
    NotAModule,
    /// A nested module could not take the view its slot declares.
    Nested,
    /// A union slot: no declared member's source side admits the value.
    NoUnionMember,
    /// A union slot: two members naming a head parameter the view hides admit the value, so
    /// neither says which the value is.
    TiedUnion,
    /// A declared type this walk has no arm for, or a value whose shape does not match the arm
    /// its declaration took. A **cyclic data value** lands here: a container that is a knot's data
    /// node is a knot member, not a container word, and nothing yet rebuilds one through a
    /// barrier.
    Unsupported(DeclaredType<Parametric>),
}

impl CoercionRefused {
    /// The refusal worded for a message, its types rendered through `types` and `symbols`.
    pub fn display<'a>(
        self,
        types: &'a TypeRegistry<'_>,
        symbols: &'a SymbolInterner,
    ) -> impl fmt::Display + 'a {
        CoercionRefusedDisplay {
            refused: self,
            types,
            symbols,
        }
    }
}

/// A [`CoercionRefused`] beside the registry and interner it renders through.
struct CoercionRefusedDisplay<'a, 'run> {
    refused: CoercionRefused,
    types: &'a TypeRegistry<'run>,
    symbols: &'a SymbolInterner,
}

impl fmt::Display for CoercionRefusedDisplay<'_, '_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = |handle: KType| display_name(handle, self.types, self.symbols);
        match self.refused {
            CoercionRefused::Seal(SealRefused::NotACarrier(identity)) => {
                write!(f, "{} is no carrier", name(identity))
            }
            CoercionRefused::Seal(SealRefused::Misfit { witness, .. }) => {
                write!(f, "its value does not satisfy {}", name(witness))
            }
            CoercionRefused::Seal(SealRefused::NotSealed { carrier }) => {
                write!(f, "it is not sealed under {}", name(carrier))
            }
            CoercionRefused::NotAFunction => f.write_str("it is no function"),
            CoercionRefused::NotAModule => f.write_str("it is no module"),
            CoercionRefused::Nested => f.write_str("its module does not fit its view"),
            CoercionRefused::NoUnionMember => f.write_str("no member of its union admits it"),
            CoercionRefused::TiedUnion => {
                f.write_str("two members of its union over hidden types admit it")
            }
            CoercionRefused::Unsupported(_) => {
                f.write_str("nothing carries a value of its shape across the view")
            }
        }
    }
}

/// What a coercion reads: the region it writes into, the lattice, and the two substitutions a
/// declared type is read under — the source module's bindings and the view's.
pub struct Coercion<'a, 'cell, 'run, 'x> {
    pub writer: Writer<'cell>,
    pub types: &'a TypeRegistry<'run>,
    pub scratch: BumpAllocator<'x>,
    /// What the source module binds the signature's head parameters to.
    pub from: Members<'x, TypeSymbol, KType>,
    /// What the view binds them to: the carriers under `:|`, the source's own under `:!`.
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
    pub(super) fn source_side(&self, declared: DeclaredType<Parametric>) -> DeclaredType<KType> {
        self.read(declared, self.from)
    }

    /// `declared` read under the view's bindings.
    pub(super) fn view_side(&self, declared: DeclaredType<Parametric>) -> DeclaredType<KType> {
        self.read(declared, self.to)
    }

    /// A `Signature` handle whose manifest members are `table` — what a barrier node holds, since
    /// a node carries `Copy`, lifetime-free handles and not a borrowed table.
    pub(super) fn sig_of(&self, table: Members<'_, TypeSymbol, KType>) -> KType {
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
        // A reference to a head parameter. Where the view side is a carrier the value takes it as
        // its one tagged layer, the barrier checking it is one and the value fits the source side —
        // a view, a view of a view, or a crossing inwards to a barrier stacked behind; where only
        // the source side is, a value sealed under it gives up its payload at the view's type.
        TypeNode::Parameter { carrier: None, .. } => {
            let (Some(src), Some(dst)) = (src_type, dst_type) else {
                return Err(unsupported);
            };
            if carrier(cx.types, dst) {
                return Tagged::seal(cx.writer, value, dst, src, cx.types, cx.scratch)
                    .map(Value::Tagged)
                    .map_err(CoercionRefused::Seal);
            }
            let not_sealed = CoercionRefused::Seal(SealRefused::NotSealed { carrier: src });
            let Value::Tagged(sealed) = value else {
                return Err(not_sealed);
            };
            if sealed.ktype() != src {
                return Err(not_sealed);
            }
            let payload = *sealed.payload();
            if !satisfies(dst, &payload, cx.types, cx.scratch) {
                return Err(not_sealed);
            }
            Ok(payload.retyped(cx.writer, dst, cx.types, cx.scratch))
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
        // Keys are untouched: a dict's key type names no parameter a view hides.
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
        // The member whose source side admits the value, by a rule blind to the union's member
        // order: where several do, the one naming a head parameter the view reads otherwise — a
        // slot declared `Carrier | Number` over a source binding `Carrier` to `Number` takes
        // `Carrier` — and two such refuse. A member naming none reads alike either side, so any
        // one of those carries the value as it is.
        TypeNode::Union { members } => {
            let carried = value.ktype();
            let mut hiding = None;
            let mut plain = None;
            for member in members.iter() {
                let declared: DeclaredType<Parametric> = member.into();
                if !satisfied_by(cx.types, cx.scratch, cx.source_side(declared), carried) {
                    continue;
                }
                if cx.source_side(declared) == cx.view_side(declared) {
                    plain = plain.or(Some(declared));
                } else if hiding.replace(declared).is_some() {
                    return Err(CoercionRefused::TiedUnion);
                }
            }
            let member = hiding.or(plain).ok_or(CoercionRefused::NoUnionMember)?;
            coerce(cx, value, member)
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
            // A nested view is the enclosing opaque one's, at the application its slot reads.
            let content =
                ModuleContent::view(Operator::Reviewed, dst, value.digest(cx.types, cx.scratch));
            view::build(
                cx.writer, member, schema, view, from, to, content, cx.types, cx.scratch,
            )
            .map(Value::Knotted)
            .map_err(|_| CoercionRefused::Nested)
        }
        _ => Err(unsupported),
    }
}

/// Whether `handle` is a carrier an opaque view hides a head parameter behind.
fn carrier(types: &TypeRegistry<'_>, handle: KType) -> bool {
    matches!(
        types.node(handle),
        TypeNode::Parameter {
            carrier: Some(_),
            ..
        }
    )
}

/// What the barrier `barrier` reads its declared type under: the source module's bindings and the
/// view's, out of the signature handles it holds — swapped for a value crossing it inwards.
fn across<'a, 'cell, 'run, 'x>(
    writer: Writer<'cell>,
    barrier: &Coerced<'_, '_>,
    inwards: bool,
    types: &'a TypeRegistry<'run>,
    scratch: BumpAllocator<'x>,
) -> Coercion<'a, 'cell, 'run, 'x> {
    let bindings = |signature: KType| {
        let schema =
            super::layout::schema_of(signature, types).expect("a barrier holds signatures");
        Members::from_pairs(
            scratch,
            schema.manifest_members.iter().map(|(name, bound)| {
                (
                    *name,
                    types.concrete(*bound).expect("a binding is concrete"),
                )
            }),
        )
    };
    let (from, to) = (bindings(barrier.from()), bindings(barrier.to()));
    let (from, to) = if inwards { (to, from) } else { (from, to) };
    Coercion {
        writer,
        types,
        scratch,
        from,
        to,
    }
}

/// `value` at the type `declared`, a slot of a barrier's declared type, read under `cx`: carried
/// as it is where both sides read it alike — a slot over the member's own `FOR ALL` group among
/// them — and coerced otherwise.
fn crossed<'graph, 'cell>(
    cx: &Coercion<'_, 'cell, '_, '_>,
    value: KValue<'graph, 'cell>,
    declared: Parametric,
) -> Result<KValue<'graph, 'cell>, CoercionRefused> {
    let (types, scratch) = (cx.types, cx.scratch);
    let src = substitute_parameters(types, scratch, declared, cx.from);
    let dst = substitute_parameters(types, scratch, declared, cx.to);
    if src == dst {
        return Ok(value);
    }
    if types.concrete(src).is_none() || types.concrete(dst).is_none() {
        return Err(CoercionRefused::Unsupported(declared.into()));
    }
    coerce(cx, value, declared.into())
}

/// `arguments`, the record a call through the barrier `barrier` was handed at the view's types,
/// coerced inwards to the types the function behind it takes: a function member's parameters by
/// name, or a keyworded member's slots under the names `parameters` binds them to, packed into
/// one list where it packs them. The record holds those fields alone, so the function behind the
/// barrier is called by name and solves its own group.
pub fn inward<'graph, 'cell>(
    writer: Writer<'cell>,
    barrier: &Coerced<'graph, 'cell>,
    arguments: KValue<'graph, 'cell>,
    parameters: ParameterBinding<'_>,
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
) -> Result<KValue<'graph, 'cell>, CoercionRefused> {
    let cx = across(writer, barrier, true, types, scratch);
    let unsupported = CoercionRefused::Unsupported(barrier.declared());
    let field = |name: BinderSymbol| {
        arguments
            .field(name.symbol(), types, scratch)
            .map(|seen| seen.value())
            .ok_or(unsupported)
    };
    let mut fields = BumpVec::new_in(scratch);
    match (declared_of(types, scratch, barrier.declared()), parameters) {
        (Declared::Function { params, .. }, _) => {
            for (name, declared) in params.iter() {
                fields.push((name, crossed(&cx, field(name)?, declared)?));
            }
        }
        (Declared::Shape { slots, .. }, ParameterBinding::Named(names)) => {
            for (name, declared) in names.iter().zip(slots.iter()) {
                fields.push((*name, crossed(&cx, field(*name)?, *declared)?));
            }
        }
        (Declared::Shape { slots, .. }, ParameterBinding::Operands) => {
            let name = BinderSymbol::Value(IMPLICIT.operands.symbol());
            let packed = field(name)?;
            let surface = packed.surface(types, scratch).ok_or(unsupported)?;
            let mut operands = BumpVec::with_capacity_in(slots.len(), scratch);
            for (at, declared) in slots.iter().enumerate() {
                let operand = surface.child(at, types, scratch).value();
                operands.push(crossed(&cx, operand, *declared)?);
            }
            let list = List::new(writer, operands.iter().copied(), types, scratch);
            fields.push((name, Value::List(list)));
        }
    }
    Ok(Value::Record(Record::new(writer, &fields, types, scratch)))
}

/// `value`, what the function behind the barrier `barrier` returned at the source's types, coerced
/// outwards to the type the view declares its return at.
pub fn outward<'graph, 'cell>(
    writer: Writer<'cell>,
    barrier: &Coerced<'graph, 'cell>,
    value: KValue<'graph, 'cell>,
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
) -> Result<KValue<'graph, 'cell>, CoercionRefused> {
    let cx = across(writer, barrier, false, types, scratch);
    let ret = match declared_of(types, scratch, barrier.declared()) {
        Declared::Function { ret, .. } | Declared::Shape { ret, .. } => ret,
    };
    crossed(&cx, value, ret)
}

/// The slot types and return of a barrier's declared type: a function type's parameters by name,
/// or an expression shape's slots in element order.
enum Declared<'run, 'x> {
    Function {
        params: TypeRecord<'run, Parametric>,
        ret: Parametric,
    },
    Shape {
        slots: BumpVec<'x, Parametric>,
        ret: Parametric,
    },
}

/// The slot types and return of the barrier's declared type `declared`.
fn declared_of<'run, 'x>(
    types: &TypeRegistry<'run>,
    scratch: BumpAllocator<'x>,
    declared: DeclaredType<Parametric>,
) -> Declared<'run, 'x> {
    let node = match declared {
        DeclaredType::Type(declared) => types.node(declared),
        DeclaredType::Scheme(scheme) => types.scheme_node(scheme),
    };
    match node {
        TypeNode::KFunction { params, ret, .. } => Declared::Function { params, ret },
        TypeNode::ExpressionShape { elements, ret, .. } => {
            let mut slots = BumpVec::new_in(scratch);
            slots.extend(elements.iter().filter_map(|element| match element {
                DispatchTokenElement::Slot(slot) => Some(slot),
                DispatchTokenElement::Keyword(_) => None,
            }));
            Declared::Shape { slots, ret }
        }
        _ => unreachable!("a barrier stands before a function"),
    }
}
