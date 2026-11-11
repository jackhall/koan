//! Members born coerced: what an opaque view does to each thing it carries across its barrier.
//!
//! Under `:|` a view's unpinned head parameters are carriers, so a member declared at one of
//! them has a type other than the one it has in the source. The member is therefore not carried
//! but rebuilt at the view's types: data is re-tagged through the admission barrier, a container
//! is rebuilt part by part from what its type shows — a record from the fields its slot declares,
//! and no other — and laid at its slot's type, a nested module is viewed — outward an opaque view
//! of its own, inward a transparent re-view — and a function is wrapped in a
//! [barrier node](super::Coerced) a call will later go through. Each type the walk reads is one
//! the load lays at a view it sees all of ([`Exposure`](super::view::Exposure)). A view is built
//! over its underlying, so the source side is the underlying's own binding, and a carrier of the
//! view's source, held by a pin or written in a signature, is read on that side through what it
//! hides ([`Hidden`]): a parameter so pinned is rebuilt along its pin, and a member at such a
//! carrier is sealed under it.
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

use crate::scope::{CoercionRefused, IMPLICIT, ParameterBinding};
use crate::type_lattice::{
    DeclaredType, DispatchTokenElement, KType, Members, Parametric, Record as TypeRecord,
    SchemaDraft, TypeNode, satisfied_by, substitute_carriers, substitute_parameters,
};
use crate::values::{Chain, Dict, List, Record, SealRefused, Tagged, Value, satisfies};

use super::{Coerced, view};

/// One side of a coercion: what a signature's head parameters are bound to, interned as a
/// signature declaring each binding a manifest member — the handle a barrier holds — beside
/// those bindings as the interned node holds them.
#[derive(Clone, Copy)]
pub struct Substitution<'run> {
    pub handle: KType,
    pub bindings: Members<'run, TypeSymbol>,
}

impl<'run> Substitution<'run> {
    /// `bindings`, interned.
    pub fn interned<'reg>(
        bindings: Members<'_, TypeSymbol, KType>,
        types: &Chain<'run, '_, 'reg, '_>,
        scratch: BumpAllocator<'_>,
    ) -> Self {
        let mut draft = SchemaDraft::new(scratch);
        for (name, kt) in bindings.iter().copied() {
            draft.insert_manifest(name, kt);
        }
        Substitution::of(types.signature(scratch, draft), types)
    }

    /// The substitution the interned `handle` holds.
    pub fn of<'reg>(handle: KType, types: &Chain<'run, '_, 'reg, '_>) -> Self {
        let schema =
            crate::elaborate::schema_of(handle, types).expect("a substitution is a signature");
        Substitution {
            handle,
            bindings: schema.manifest_members,
        }
    }
}

/// The carriers a view's types show, each beside the type it hides over the module the view is
/// built over, and whether that module's side is a coercion's `from` side — outward — or its `to`
/// side, crossing a barrier inward. A type read on that side reads each carrier as what it hides.
#[derive(Clone, Copy)]
pub struct Hidden<'a> {
    pub carriers: &'a [(KType, KType)],
    pub on_from: bool,
}

impl<'a> Hidden<'a> {
    /// No carrier read back.
    pub const NONE: Hidden<'static> = Hidden {
        carriers: &[],
        on_from: true,
    };

    /// These carriers read on the other side: a crossing the other way.
    pub fn flipped(self) -> Self {
        Hidden {
            on_from: !self.on_from,
            ..self
        }
    }

    /// The type `carrier` hides, where it is one of these.
    pub fn of(&self, carrier: KType) -> Option<KType> {
        crate::scope::hides(self.carriers, carrier)
    }

    /// These carriers laid in `writer`'s region.
    pub(super) fn laid<'to>(self, writer: Writer<'to, '_>) -> Hidden<'to> {
        Hidden {
            carriers: crate::memory::collect(writer, self.carriers.iter().copied()),
            on_from: self.on_from,
        }
    }
}

/// What a coercion reads: the region it writes into, the lattice, and the two substitutions a
/// declared type is read under — the source module's bindings and the view's.
pub struct Coercion<'a, 'tier, 'cell, 'x, 'reg> {
    pub writer: Writer<'cell, 'a>,
    pub types: &'a Chain<'tier, 'cell, 'reg, 'a>,
    pub scratch: BumpAllocator<'x>,
    /// What the source module binds the signature's head parameters to.
    pub from: Substitution<'tier>,
    /// What the view binds them to: the carriers under `:|`, the source's own under `:!`.
    pub to: Substitution<'tier>,
    /// Whether the walk crosses a barrier inward, where a nested module is re-viewed
    /// transparently; outward it is viewed opaquely.
    pub inward: bool,
    /// The view's carriers, read back on the underlying's side.
    pub hidden: Hidden<'a>,
}

/// Why a member type read under a module's or a view's bindings is concrete: it reads only its
/// signature's head parameters, and each is bound to a concrete type.
const BOUND: &str = "a member type reads only head parameters, each bound";

impl<'cell, 'x> Coercion<'_, '_, 'cell, 'x, '_> {
    /// `declared` read under the `from` bindings where `from`, else the `to` ones: on the
    /// underlying's side, each of the view's carriers as what it hides.
    fn side(&self, declared: DeclaredType<Parametric>, from: bool) -> DeclaredType<Parametric> {
        let (types, scratch) = (self.types, self.scratch);
        let bindings = if from {
            self.from.bindings
        } else {
            self.to.bindings
        };
        let read = substitute_parameters(types, scratch, declared, bindings);
        if self.hidden.on_from != from || self.hidden.carriers.is_empty() {
            return read;
        }
        substitute_carriers(types, scratch, read, &mut |carrier| self.hidden.of(carrier))
    }

    /// `declared` read under the source module's bindings.
    pub(super) fn source_side(&self, declared: DeclaredType<Parametric>) -> DeclaredType<KType> {
        (self.types.concrete_declared(self.side(declared, true))).expect(BOUND)
    }

    /// `declared` read under the view's bindings.
    pub(super) fn view_side(&self, declared: DeclaredType<Parametric>) -> DeclaredType<KType> {
        (self.types.concrete_declared(self.side(declared, false))).expect(BOUND)
    }
}

/// `value`, which the source module holds at `declared` read under `cx.from`, rebuilt at
/// `declared` read under `cx.to`: carried as it is where both sides read it alike — a slot over the
/// member's own `FOR ALL` group among them — and refused where they differ over such a group.
pub fn coerce<'graph, 'cell>(
    cx: &Coercion<'_, '_, 'cell, '_, '_>,
    value: KValue<'graph, 'cell>,
    declared: DeclaredType<Parametric>,
) -> Result<KValue<'graph, 'cell>, CoercionRefused> {
    let types = cx.types;
    let (src, dst) = (cx.side(declared, true), cx.side(declared, false));
    // The two sides agree, so the member already has the type the view declares: the whole of
    // `:!`, and every slot of `:|` that names no unpinned parameter.
    if src == dst {
        return Ok(value);
    }
    let unsupported = CoercionRefused::Unsupported(declared);
    // A side still naming a variable — a slot over a member's own `FOR ALL` group — has no type
    // to rebuild the value at.
    let (Some(src), Some(dst)) = (types.concrete_declared(src), types.concrete_declared(dst))
    else {
        return Err(unsupported);
    };
    let node = declared.view(cx.types).body;
    // Only a function slot is a scheme, so every other arm's two sides are types.
    let (src_type, dst_type) = (src.as_type(), dst.as_type());
    match node {
        // A reference to a head parameter. Where the view side is a carrier the value is sealed
        // under it, the barrier checking it is one and the value fits the source side: the
        // underlying's binding, which may itself be a value another view sealed, as
        // `MODULE n = ((LET zero = c.zero))` binds one of `c`'s. Where only the source side is — a
        // value crossing a barrier inward — a value sealed under it gives up its payload at the
        // view's type.
        // A carrier is declared inside a pin the walk descends into, or by a signature naming one.
        TypeNode::Parameter { .. } | TypeNode::Carrier { .. } => {
            let (Some(src), Some(dst)) = (src_type, dst_type) else {
                return Err(unsupported);
            };
            if cx.types.is_carrier(dst) {
                return Tagged::seal(cx.writer, value, dst, src, cx.types, cx.scratch)
                    .map(Value::Tagged)
                    .map_err(CoercionRefused::Seal);
            }
            // A parameter pinned at a type holding the view's carriers: rebuilt along the pin,
            // read as written on the view's side and through its carriers on the underlying's.
            if !cx.types.is_carrier(src) {
                let pin = if cx.hidden.on_from { dst } else { src };
                return coerce(cx, value, DeclaredType::Type(pin.into()));
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
            Ok(Value::List(List::typed(
                cx.writer,
                cells.iter().copied(),
                dst,
                cx.types,
            )))
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
            Ok(Value::Dict(Dict::typed(
                cx.writer, &entries, dst, cx.types, cx.scratch,
            )))
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
            Ok(Value::Record(Record::typed(
                cx.writer, &built, dst, cx.types, cx.scratch,
            )))
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
        // A header's own type is superseded by the barrier's, so the barrier stands before its
        // bare member.
        TypeNode::KFunction { .. } | TypeNode::ExpressionShape { .. } => {
            let Some(member) = value.knot_member() else {
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
                cx.from.handle,
                cx.to.handle,
                cx.hidden,
                cx.types,
            );
            Ok(Value::Knotted(Knotted::of(knot, 0)))
        }
        // A nested module. Outward it is an opaque view of the underlying's member at the slot's
        // application read on the view's side, admitted at it read on the underlying's: the
        // enclosing carriers arrive through the pins, and each parameter the application leaves
        // unpinned gets a carrier of the nested view's. Inward it is re-viewed transparently, the
        // re-view its own underlying. An application naming none of the enclosing parameters
        // never reaches here: its two sides agree, and the comparison above carried the module. A
        // bare signature is the application of it that pins nothing, and reaches here where it
        // names an enclosing carrier.
        TypeNode::Signature { .. } | TypeNode::SignatureApply { .. } => {
            let Some(member) = value
                .knot_member()
                .filter(|member| member.module().is_some())
            else {
                return Err(CoercionRefused::NotAModule);
            };
            let (Some(src), Some(dst)) = (src_type, dst_type) else {
                return Err(unsupported);
            };
            // A header's carried application is what the module offers the nested slot.
            let held = value
                .ktype()
                .as_type()
                .expect("a module's type is a signature type");
            let exposure = if cx.inward {
                let bare = Value::Knotted(member).digest(cx.types, cx.scratch);
                view::nested_types(held, src, dst, bare, cx.types, cx.scratch)
            } else {
                let operand = view::operand_of(Some(held), member, cx.types, cx.scratch);
                let context = cx.hidden.carriers;
                view::exposure(operand, dst, src, context, cx.types, cx.scratch)
            }
            .map_err(|_| CoercionRefused::Nested)?;
            let built = match cx.inward {
                true => {
                    let (Some(from), Some(built)) = (exposure.from, exposure.built) else {
                        unreachable!("a re-view's fit binds both sides")
                    };
                    let (sig, writer) = (exposure.sig, cx.writer);
                    view::build(
                        writer, value, sig, from, built, None, None, true, cx.hidden, cx.types,
                        cx.scratch,
                    )
                }
                false => view::built_over(
                    cx.writer, member, &exposure, None, false, cx.types, cx.scratch,
                ),
            };
            built
                .map(Value::Knotted)
                .map_err(|_| CoercionRefused::Nested)
        }
        _ => Err(unsupported),
    }
}

/// What the barrier `barrier` reads its declared type under: the source module's bindings and the
/// view's, read off the signature handles it holds — swapped for a value crossing it inwards.
fn across<'a, 'tier, 'cell, 'x, 'reg>(
    writer: Writer<'cell, 'a>,
    barrier: &'a Coerced<'_, 'a>,
    inwards: bool,
    types: &'a Chain<'tier, 'cell, 'reg, '_>,
    scratch: BumpAllocator<'x>,
) -> Coercion<'a, 'tier, 'cell, 'x, 'reg> {
    let (from, to) = (
        Substitution::of(barrier.from(), types),
        Substitution::of(barrier.to(), types),
    );
    let (from, to) = if inwards { (to, from) } else { (from, to) };
    let hidden = barrier.hidden();
    Coercion {
        writer,
        types,
        scratch,
        from,
        to,
        inward: inwards,
        hidden: if inwards { hidden.flipped() } else { hidden },
    }
}

/// `arguments`, the record a call through the barrier `barrier` was handed at the view's types,
/// coerced inwards to the types the function behind it takes: a function member's parameters by
/// name, or a keyworded member's slots under the names `parameters` binds them to, packed into
/// one list where it packs them. The record holds those fields alone, so the function behind the
/// barrier is called by name and solves its own group.
pub fn inward<'graph, 'cell>(
    writer: Writer<'cell, '_>,
    barrier: &Coerced<'graph, 'cell>,
    arguments: KValue<'graph, 'cell>,
    parameters: ParameterBinding<'_>,
    types: &Chain<'_, 'cell, '_, '_>,
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
                fields.push((name, coerce(&cx, field(name)?, declared.into())?));
            }
        }
        (Declared::Shape { slots, .. }, ParameterBinding::Named(names)) => {
            for (name, declared) in names.iter().zip(slots.iter()) {
                fields.push((*name, coerce(&cx, field(*name)?, (*declared).into())?));
            }
        }
        (Declared::Shape { slots, .. }, ParameterBinding::Operands) => {
            let name = BinderSymbol::Value(IMPLICIT.operands.symbol());
            let packed = field(name)?;
            let surface = packed.surface(types, scratch).ok_or(unsupported)?;
            let mut operands = BumpVec::with_capacity_in(slots.len(), scratch);
            for (at, declared) in slots.iter().enumerate() {
                let operand = surface.child(at, types, scratch).value();
                operands.push(coerce(&cx, operand, (*declared).into())?);
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
    writer: Writer<'cell, '_>,
    barrier: &Coerced<'graph, 'cell>,
    value: KValue<'graph, 'cell>,
    types: &Chain<'_, 'cell, '_, '_>,
    scratch: BumpAllocator<'_>,
) -> Result<KValue<'graph, 'cell>, CoercionRefused> {
    let cx = across(writer, barrier, false, types, scratch);
    let ret = match declared_of(types, scratch, barrier.declared()) {
        Declared::Function { ret, .. } | Declared::Shape { ret, .. } => ret,
    };
    coerce(&cx, value, ret.into())
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
    types: &Chain<'run, '_, '_, '_>,
    scratch: BumpAllocator<'x>,
    declared: DeclaredType<Parametric>,
) -> Declared<'run, 'x> {
    match declared.view(types).body {
        TypeNode::KFunction { params, ret } => Declared::Function { params, ret },
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
