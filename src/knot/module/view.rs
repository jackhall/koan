//! The view door: what `m :! Sig` and `m :| Sig` build.
//!
//! A view is a module of its own — the same node, through the same constructor — holding only the
//! members its signature names, at the types that signature declares. The ascribed type is one
//! application of a declared signature; a meet of several is refused. The source must *fit* it
//! ([`fits_application`]), which solves each head parameter the application leaves unpinned from
//! what the source's members offer. Both operators narrow; they differ in what the parameters mean
//! afterwards.
//!
//! Under `:!` they mean what *fits* solved them to, or what the application pins them to, so the
//! view's members are the source's own words and the view is a relabelling. Under `:|` each
//! unpinned one is hidden behind a **carrier keyed on content**: a [`Carrier`](TypeNode::Carrier)
//! keyed on the view's own [content](ModuleContent::carrier_key) — its operator and application
//! over the source's digest — so two ascriptions of modules of equal content share their carriers
//! and two of other content never unify, however often either runs. Every member is then born
//! [coerced](super::coerce) to the carriers. A pinned parameter keeps its pin either way.
//!
//! A view carries a keyworded member too: past its named members, each overload the source offers
//! at the member's key that the member read under the source's bindings admits,
//! [coerced](super::coerce::coerce) as any member is — behind a barrier where the view reads the
//! member otherwise.
//!
//! No carrier is made at a *nested* boundary. A slot declared at an application whose pins name
//! the outer signature's parameters is re-viewed against it read under the outer view's bindings,
//! so the nested view's identities are the outer carriers, arriving through the declared type
//! rather than being made again. [`fitted`] and [`build`] are the one fit and the one body both
//! the outer ascription and the nested case go through.
//!
//! [`view_types`] is the half of an ascription computed from types alone: the bindings either side
//! and the view's signature. The load calls it at a `:|` site over a content tree it sees all of,
//! so the root holds each type the run's view would otherwise intern; [`nested_types`] is the
//! same half of a nested re-view, which the load lays wherever it knows what a nested module
//! carries (`dispatch/statics/views.rs`).

use crate::knot::{KValue, Knotted};
use crate::memory::{BumpAllocator, BumpVec, Writer};
use crate::symbols::{BinderSymbol, TypeSymbol, ValueSymbol};
use crate::type_lattice::{
    ContentKey, FitsFailure, KKind, KType, Members, Parametric, SchemaDraft, SigSchema, TypeNode,
    fits_application, member as bound_member, satisfied_by, substitute_parameters,
};
use crate::values::{ContentDigest, TypeValue, Types, Value};

use super::coerce::{Coercion, CoercionRefused, Substitution, coerce};
use super::{Module, ModuleContent, Operator, layout};

/// Which operator is ascribing: `:!` keeps the source's types, `:|` hides them behind carriers. The
/// two of the three [`Operator`]s a program writes; the third is a nested re-view's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ascription {
    /// `:|` — each unpinned head parameter is hidden behind a carrier, and every member is born coerced
    /// to it.
    Opaque,
    /// `:!` — the parameters keep what *fits* solved them to, so every member is carried verbatim.
    Transparent,
}

/// Why a module could not take a signature.
#[derive(Clone, Copy, Debug)]
pub enum Unascribable<'run, 'x> {
    /// The operand is no module.
    NotAModule,
    /// The ascribed handle names no one application of a signature.
    NotASignature(KType),
    /// The module does not fit the signature.
    Unsatisfied(FitsFailure<'run, 'x>),
    /// A member the signature names could not take the view's type for it.
    Coercion {
        name: ValueSymbol,
        refused: CoercionRefused,
    },
}

/// Whether the view door takes `ascribed`: one application of a signature, or the signature.
pub fn takes(types: &Types<'_, '_>, ascribed: KType) -> bool {
    matches!(
        types.node(ascribed),
        TypeNode::Signature { .. } | TypeNode::SignatureApply { .. }
    )
}

/// Whether an ascription by `mode` at `ascribed` of an operand of type `operand` views it — builds
/// the module seen through a signature — rather than holding it to a type: `:|` always does, `:!`
/// where both are signature types. The run reads it over the carried type; the load over the
/// static upper end, both types read through their bounds.
pub fn views(types: &Types<'_, '_>, mode: Ascription, operand: KType, ascribed: KType) -> bool {
    match mode {
        Ascription::Opaque => true,
        Ascription::Transparent => {
            operand.kind_of(types) == KKind::Signature
                && ascribed.kind_of(types) == KKind::Signature
        }
    }
}

/// `source` seen as `signature`, as a module of its own. A refusal writes nothing but what a
/// partial coercion walk had already laid down, which nothing names.
pub fn ascribe<'graph, 'cell: 'x, 'x>(
    writer: Writer<'cell>,
    source: Knotted<'graph, 'cell>,
    signature: KType,
    mode: Ascription,
    types: &Types<'_, 'cell>,
    scratch: BumpAllocator<'x>,
) -> Result<Knotted<'graph, 'cell>, Unascribable<'cell, 'x>> {
    let held = source.module().ok_or(Unascribable::NotAModule)?.ktype();
    // A view is its operator and its application over its source. A module's digest is its knot's
    // over the content it was born with: no walk.
    let operator = match mode {
        Ascription::Transparent => Operator::Transparent,
        Ascription::Opaque => Operator::Opaque,
    };
    let source_digest = Value::Knotted(source).digest(types, scratch);
    let content = ModuleContent::view(operator, signature, source_digest);
    let viewed = view_types(
        Some(held),
        signature,
        mode,
        content.carrier_key(),
        types,
        scratch,
    )?;
    let from = viewed
        .from
        .expect("a held signature binds the source's side");
    build(
        writer,
        source,
        viewed.sig,
        viewed.view,
        from,
        viewed.to,
        content,
        types,
        scratch,
    )
}

/// The types a view at `signature` of a module of signature `held` carries, computed from types
/// alone — everything [`ascribe`] interns before it builds a member. The load computes them at a
/// `:|` site over a content tree it sees all of, so the run finds each in the root.
#[derive(Clone, Copy)]
pub struct ViewTypes<'run> {
    /// The source module's signature, where known.
    pub held: Option<KType>,
    /// The ascribed signature's schema.
    pub sig: SigSchema<'run>,
    /// What the source binds the head parameters to: what *fits* solves. `None` where `held` was.
    pub from: Option<Substitution<'run>>,
    /// What the view binds them to: the carriers under `:|`, the source's own under `:!`.
    pub to: Substitution<'run>,
    /// The view's own signature.
    pub view: KType,
}

/// What a view at `signature` of a module whose signature is `held` binds, its carriers keyed on
/// `key`, the key of the view's content. Without `held` only an opaque view's own side: each
/// carrier and each pin, which is all [`carriers`] reads of the source's side. Refused where
/// `signature` names no one application of a signature, or `held` does not fit it.
pub fn view_types<'run: 'x, 'x>(
    held: Option<KType>,
    signature: KType,
    mode: Ascription,
    key: ContentKey,
    types: &Types<'_, 'run>,
    scratch: BumpAllocator<'x>,
) -> Result<ViewTypes<'run>, Unascribable<'run, 'x>> {
    debug_assert!(
        held.is_some() || mode == Ascription::Opaque,
        "a transparent view binds what its source does"
    );
    let (sig, from, pins) = match held {
        Some(held) => fitted(held, signature, types, scratch)?,
        None => {
            let (sig, _, pins) = applied(signature, types, scratch)?;
            // `carriers` reads a pinned parameter's binding from what fits solved, which is its pin.
            let from = Members::from_pairs(
                scratch,
                pins.iter().filter_map(|(name, pin)| match name {
                    BinderSymbol::Type(name) => Some((*name, *pin)),
                    _ => None,
                }),
            );
            (sig, from, pins)
        }
    };
    let to = match mode {
        // Transparent: the parameters keep the source's bindings, so every slot type reads the
        // same either side and the coercion walk stops at its first comparison.
        Ascription::Transparent => from,
        // A carrier is keyed on the view's content. Two views of equal content share one, and the
        // parameter's name keeps two of one view apart.
        Ascription::Opaque => carriers(&sig, from, &pins, key, types, scratch),
    };
    Ok(ViewTypes {
        held,
        sig,
        from: held.map(|_| Substitution::interned(from, types, scratch)),
        to: Substitution::interned(to, types, scratch),
        view: view_signature(&sig, to, types, scratch),
    })
}

/// What a nested module of signature `held` binds when re-viewed at a slot whose application
/// reads `src` under the source's bindings and `dst` under the view's: *fits* solves the source's
/// side against `src`, and the view's side takes each parameter `dst` pins, the rest kept as
/// solved. The coercion walk and the load's lay of a view both compute it here.
pub(crate) fn nested_types<'run: 'x, 'x>(
    held: KType,
    src: KType,
    dst: KType,
    types: &Types<'_, 'run>,
    scratch: BumpAllocator<'x>,
) -> Result<ViewTypes<'run>, Unascribable<'run, 'x>> {
    let TypeNode::SignatureApply { pins, .. } = types.node(dst) else {
        return Err(Unascribable::NotASignature(dst));
    };
    let (sig, from, _) = fitted(held, src, types, scratch)?;
    let to = Members::from_pairs(
        scratch,
        from.iter().map(|(name, solved)| {
            let pinned = pins.get(BinderSymbol::Type(*name).symbol());
            (*name, pinned.unwrap_or(*solved))
        }),
    );
    Ok(ViewTypes {
        held: Some(held),
        sig,
        from: Some(Substitution::interned(from, types, scratch)),
        to: Substitution::interned(to, types, scratch),
        view: view_signature(&sig, to, types, scratch),
    })
}

/// What a transparent view at `signature` of a module whose signature is `source` binds: the
/// source's bindings either side, and the signature the view carries — what the load types
/// `m :! Sig` as, where it knows `m`'s signature exactly. The very handles [`ascribe`] lays down,
/// since both are one computation. `None` where the view door would refuse.
pub fn transparent_view_types<'run>(
    source: KType,
    signature: KType,
    types: &Types<'_, 'run>,
    scratch: BumpAllocator<'_>,
) -> Option<ViewTypes<'run>> {
    let (sig, from, _) = fitted(source, signature, types, scratch).ok()?;
    let interned = Substitution::interned(from, types, scratch);
    Some(ViewTypes {
        held: Some(source),
        sig,
        from: Some(interned),
        to: interned,
        view: view_signature(&sig, from, types, scratch),
    })
}

/// What an opaque view at `signature` of a module whose signature is `held`, where known, binds
/// over a source whose digest is `source`, computed from types alone, as the load computes it over
/// a source it sees all of: each carrier keyed on the view's content and each pinned parameter at
/// its pin. The very handles [`ascribe`] lays down where the source fits; the view's signature
/// needs no `held`. `None` where the view door would refuse.
pub fn opaque_view_types<'run>(
    held: Option<KType>,
    signature: KType,
    source: ContentDigest,
    types: &Types<'_, 'run>,
    scratch: BumpAllocator<'_>,
) -> Option<ViewTypes<'run>> {
    let key = ModuleContent::view(Operator::Opaque, signature, source).carrier_key();
    view_types(held, signature, Ascription::Opaque, key, types, scratch).ok()
}

/// The ascribed signature's schema, what a module of signature `held` binds its head parameters
/// to under *fits*, and the application's pins. Refused where `signature` names no one application
/// of a signature, or `held` does not fit it.
#[allow(clippy::type_complexity)]
pub(super) fn fitted<'run: 'x, 'x>(
    held: KType,
    signature: KType,
    types: &Types<'_, 'run>,
    scratch: BumpAllocator<'x>,
) -> Result<
    (
        SigSchema<'run>,
        Members<'x, TypeSymbol, KType>,
        BumpVec<'x, (BinderSymbol, KType)>,
    ),
    Unascribable<'run, 'x>,
> {
    let (sig, declared, pins) = applied(signature, types, scratch)?;
    // The empty signature asks nothing, and every module fits it.
    let from = if sig.is_empty() {
        Members::EMPTY
    } else {
        let solution = fits_application(types, scratch, held, declared, &pins)
            .map_err(Unascribable::Unsatisfied)?;
        solved(types, scratch, solution)
    };
    Ok((sig, from, pins))
}

/// The ascribed signature's schema, the signature it applies, and the application's pins. Refused
/// where `signature` names no one application of a signature.
#[allow(clippy::type_complexity)]
fn applied<'run: 'x, 'x>(
    signature: KType,
    types: &Types<'_, 'run>,
    scratch: BumpAllocator<'x>,
) -> Result<(SigSchema<'run>, KType, BumpVec<'x, (BinderSymbol, KType)>), Unascribable<'run, 'x>> {
    if !takes(types, signature) {
        return Err(Unascribable::NotASignature(signature));
    }
    let mut pins = BumpVec::new_in(scratch);
    let declared = match types.node(signature) {
        TypeNode::SignatureApply {
            signature,
            pins: pinned,
        } => {
            pins.extend(pinned.iter());
            signature
        }
        _ => signature,
    };
    let sig = crate::elaborate::schema_of(declared, types)
        .ok_or(Unascribable::NotASignature(signature))?;
    Ok((sig, declared, pins))
}

/// What *fits* solved a module's own signature against an application to. A module's
/// self-signature declares no head parameter, so no offered stand-in reaches the solution, and
/// each parameter is solved to a concrete type.
fn solved<'x>(
    types: &Types<'_, '_>,
    scratch: BumpAllocator<'x>,
    solution: Members<'_, TypeSymbol, Parametric>,
) -> Members<'x, TypeSymbol, KType> {
    Members::from_pairs(
        scratch,
        solution.iter().map(|(name, solved)| {
            let solved = types
                .concrete(*solved)
                .expect("a module's signature offers no stand-in to what fits solves");
            (*name, solved)
        }),
    )
}

/// Build the view's member run in layout order and lay the node down, its content `content`. `from`
/// and `to` are what the source and the view bind the signature's head parameters to, interned.
///
/// Two callers: an ascription, and a nested signature slot inside one
/// ([`coerce`](super::coerce::coerce)), which passes the enclosing substitutions unchanged —
/// a nested boundary makes no carrier of its own. Satisfaction is the caller's: an ascription checks
/// it outright, and a nested slot was checked when the enclosing one was.
#[allow(clippy::too_many_arguments)]
pub(super) fn build<'graph, 'cell, 'x>(
    writer: Writer<'cell>,
    source: Knotted<'graph, 'cell>,
    sig: SigSchema<'cell>,
    view: KType,
    from: Substitution<'cell>,
    to: Substitution<'cell>,
    content: ModuleContent,
    types: &Types<'_, 'cell>,
    scratch: BumpAllocator<'x>,
) -> Result<Knotted<'graph, 'cell>, Unascribable<'cell, 'x>> {
    let cx = Coercion {
        writer,
        types,
        scratch,
        from,
        to,
    };
    let view_schema = crate::elaborate::schema_of(view, types).expect("the view's own signature");
    let mut members: BumpVec<'x, KValue<'graph, 'cell>> = BumpVec::with_capacity_in(
        crate::elaborate::member_count(&view_schema, scratch),
        scratch,
    );
    // Value members first, in the signature's own order — which is layout order, since both
    // schemas' value tables hold the same names symbol-sorted.
    for (name, declared) in sig.value_slots.iter().copied() {
        let held = layout::member(source, BinderSymbol::Value(name), types, scratch)
            .expect("satisfaction admitted the member, so the source holds it");
        let member = coerce(&cx, held, declared)
            .map_err(|refused| Unascribable::Coercion { name, refused })?;
        members.push(member);
    }
    // Then the type members, at the handles the view's own schema fixed them to.
    // A view's signature is a module's: every type member is fixed to a concrete type.
    for (_, handle) in crate::elaborate::type_members(&view_schema, scratch)
        .iter()
        .copied()
    {
        let handle = types
            .concrete(handle)
            .expect("a view's type members are bound");
        members.push(Value::Type(TypeValue::new(writer, handle, types)));
    }
    // Then, per keyworded member, each overload the source offers at its key that the member
    // read under `from` admits — behind a barrier where the view reads the member otherwise.
    for declared in sig.keyworded.iter().copied() {
        let src = cx.source_side(declared);
        for function in
            layout::functions_at(source, layout::key_of(declared, types), types, scratch)
        {
            let registered = layout::registered_shape(function).expect("a function");
            if !satisfied_by(types, scratch, src, registered) {
                continue;
            }
            members.push(
                coerce(&cx, function, declared)
                    .expect("a registration member is a function, which a barrier takes"),
            );
        }
    }
    Ok(Knotted::of(Module::tie(writer, view, &members, content), 0))
}

/// The view's bindings under `:|`: a carrier per head parameter of `sig` that `pins` leaves
/// unpinned — a [`Carrier`](TypeNode::Carrier) keyed on `key`, under the parameter's name, meeting
/// the bound the declaration gives it — and each pinned one at what `from` holds for it, its pin.
fn carriers<'x>(
    sig: &SigSchema<'_>,
    from: Members<'_, TypeSymbol, KType>,
    pins: &[(BinderSymbol, KType)],
    key: ContentKey,
    types: &Types<'_, '_>,
    scratch: BumpAllocator<'x>,
) -> Members<'x, TypeSymbol, KType> {
    Members::from_pairs(
        scratch,
        sig.parameters.iter().map(|(name, declared)| {
            let pinned = pins
                .iter()
                .any(|(pin, _)| *pin == BinderSymbol::Type(*name));
            let to = if pinned {
                bound_member(from, *name).expect("fits binds every parameter")
            } else {
                let bound = types.node(*declared).rigid_bound().unwrap_or(KType::ANY);
                types.carrier(*name, bound, key)
            };
            (*name, to)
        }),
    )
}

/// The signature the view itself carries: each of `sig`'s parameters a manifest member at what `to`
/// gives it, each manifest member, value slot and keyworded member at its declared type read under
/// `to`. A view's signature is a module's, so it has no parameters, and it fits the signature it
/// was ascribed to.
pub(super) fn view_signature(
    sig: &SigSchema<'_>,
    to: Members<'_, TypeSymbol, KType>,
    types: &Types<'_, '_>,
    scratch: BumpAllocator<'_>,
) -> KType {
    let mut draft = SchemaDraft::new(scratch);
    for (name, _) in sig.parameters.iter().copied() {
        draft.insert_manifest(
            name,
            bound_member(to, name).expect("every parameter is bound"),
        );
    }
    for (name, fixed) in sig.manifest_members.iter().copied() {
        draft.insert_manifest(name, substitute_parameters(types, scratch, fixed, to));
    }
    for (name, declared) in sig.value_slots.iter().copied() {
        draft.insert_value_slot(name, substitute_parameters(types, scratch, declared, to));
    }
    for declared in sig.keyworded.iter().copied() {
        draft.push_keyworded(substitute_parameters(types, scratch, declared, to));
    }
    types.signature(scratch, draft)
}
