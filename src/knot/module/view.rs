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

use crate::knot::{KValue, Knotted};
use crate::memory::{BumpAllocator, BumpVec, Writer};
use crate::symbols::{BinderSymbol, TypeSymbol, ValueSymbol};
use crate::type_lattice::{
    ContentKey, FitsFailure, KType, Members, Parametric, SchemaDraft, SigSchema, TypeNode,
    TypeRegistry, fits_application, member as bound_member, satisfied_by, substitute_parameters,
};
use crate::values::{TypeValue, Value};

use super::coerce::{Coercion, CoercionRefused, coerce};
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

/// Whether `ktype` is a signature type — a signature, an application or a meet of them — which
/// only a module carries or satisfies.
pub fn is_signature_type(types: &TypeRegistry<'_>, ktype: KType) -> bool {
    matches!(
        types.node(ktype),
        TypeNode::Signature { .. }
            | TypeNode::SignatureApply { .. }
            | TypeNode::SignatureMeet { .. }
    )
}

/// Whether the view door takes `ascribed`: one application of a signature, or the signature.
pub fn takes(types: &TypeRegistry<'_>, ascribed: KType) -> bool {
    matches!(
        types.node(ascribed),
        TypeNode::Signature { .. } | TypeNode::SignatureApply { .. }
    )
}

/// Whether an ascription by `mode` at `ascribed` of an operand of type `operand` views it — builds
/// the module seen through a signature — rather than holding it to a type: `:|` always does, `:!`
/// where both are signature types. The run reads it over the carried type; the load over the
/// static upper end, both types read through their bounds.
pub fn views(types: &TypeRegistry<'_>, mode: Ascription, operand: KType, ascribed: KType) -> bool {
    match mode {
        Ascription::Opaque => true,
        Ascription::Transparent => {
            is_signature_type(types, operand) && is_signature_type(types, ascribed)
        }
    }
}

/// `source` seen as `signature`, as a module of its own. A refusal writes nothing but what a
/// partial coercion walk had already laid down, which nothing names.
pub fn ascribe<'graph, 'cell, 'run, 'x>(
    writer: Writer<'cell>,
    source: Knotted<'graph, 'cell>,
    signature: KType,
    mode: Ascription,
    types: &TypeRegistry<'run>,
    scratch: BumpAllocator<'x>,
) -> Result<Knotted<'graph, 'cell>, Unascribable<'run, 'x>> {
    let held = source.module().ok_or(Unascribable::NotAModule)?.ktype();
    let (sig, from, pins) = fitted(held, signature, types, scratch)?;
    // A view is its operator and its application over its source. A module's digest is its knot's
    // over the content it was born with: no walk.
    let operator = match mode {
        Ascription::Transparent => Operator::Transparent,
        Ascription::Opaque => Operator::Opaque,
    };
    let source_digest = Value::Knotted(source).digest(types, scratch);
    let content = ModuleContent::view(operator, signature, source_digest);
    let to = match mode {
        // Transparent: the parameters keep the source's bindings, so every slot type reads the
        // same either side and the coercion walk stops at its first comparison.
        Ascription::Transparent => from,
        // A carrier is keyed on the view's content. Two views of equal content share one, and the
        // parameter's name keeps two of one view apart.
        Ascription::Opaque => carriers(&sig, from, &pins, content.carrier_key(), types, scratch),
    };
    let view = view_signature(&sig, to, types, scratch);
    build(writer, source, sig, view, from, to, content, types, scratch)
}

/// The signature a transparent view of a module whose signature is `source` carries, seen as
/// `signature` — what the load types `m :! Sig` as, where it knows `m`'s signature exactly. The
/// very handle [`ascribe`] lays down, since both are one computation. `None` where the view door
/// would refuse.
pub fn transparent_view_type(
    source: KType,
    signature: KType,
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
) -> Option<KType> {
    let (sig, from, _) = fitted(source, signature, types, scratch).ok()?;
    Some(view_signature(&sig, from, types, scratch))
}

/// The ascribed signature's schema, what a module of signature `held` binds its head parameters
/// to under *fits*, and the application's pins. Refused where `signature` names no one application
/// of a signature, or `held` does not fit it.
#[allow(clippy::type_complexity)]
pub(super) fn fitted<'run, 'x>(
    held: KType,
    signature: KType,
    types: &TypeRegistry<'run>,
    scratch: BumpAllocator<'x>,
) -> Result<
    (
        SigSchema<'run>,
        Members<'x, TypeSymbol, KType>,
        BumpVec<'x, (BinderSymbol, KType)>,
    ),
    Unascribable<'run, 'x>,
> {
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

/// What *fits* solved a module's own signature against an application to. A module's
/// self-signature declares no head parameter, so no offered stand-in reaches the solution, and
/// each parameter is solved to a concrete type.
fn solved<'x>(
    types: &TypeRegistry<'_>,
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
/// and `to` are what the source and the view bind the signature's head parameters to.
///
/// Two callers: an ascription, and a nested signature slot inside one
/// ([`coerce`](super::coerce::coerce)), which passes the enclosing substitutions unchanged —
/// a nested boundary makes no carrier of its own. Satisfaction is the caller's: an ascription checks
/// it outright, and a nested slot was checked when the enclosing one was.
#[allow(clippy::too_many_arguments)]
pub(super) fn build<'graph, 'cell, 'run, 'x>(
    writer: Writer<'cell>,
    source: Knotted<'graph, 'cell>,
    sig: SigSchema<'run>,
    view: KType,
    from: Members<'x, TypeSymbol, KType>,
    to: Members<'x, TypeSymbol, KType>,
    content: ModuleContent,
    types: &TypeRegistry<'run>,
    scratch: BumpAllocator<'x>,
) -> Result<Knotted<'graph, 'cell>, Unascribable<'run, 'x>> {
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
    types: &TypeRegistry<'_>,
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
    types: &TypeRegistry<'_>,
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
