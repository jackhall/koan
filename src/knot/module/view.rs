//! The view door: what `m :| Sig` builds. (`m :! Sig` is a retype, which re-stamps the module at
//! the application it fits under a header, and never comes here.)
//!
//! A view is a module of its own — the same node, through the same constructor — holding only the
//! members its signature names, at the types that signature declares. The ascribed type is one
//! application of a declared signature; a meet of several is refused. The source must *fit* it
//! ([`fit_solution`]), which solves each head parameter the application leaves unpinned from
//! what the source's members offer. The source's signature is the type it carries: a header's
//! application, or a bare module's own.
//!
//! Each unpinned parameter is hidden behind a **carrier keyed on content**: a
//! [`Carrier`](TypeNode::Carrier) keyed on the view's own [content](ModuleContent::carrier_key) —
//! its operator and application over its source's *bare member* digest and what *fits* solved —
//! so two ascriptions over one module that fit alike share their carriers, through a header or
//! not, and two of other content never unify, however often either runs. Every member is then born
//! [coerced](super::coerce) to the carriers. A pinned parameter keeps its pin.
//!
//! A view carries a keyworded member too: past its named members, the overloads the source offers
//! at the member's key that cover the member read under the source's bindings, narrowed to the
//! most specific ([`layout::covering`], the pick a header makes), each
//! [coerced](super::coerce::coerce) as any member is — behind a barrier where the view reads the
//! member otherwise — and retyped to the member's shape the view shows.
//!
//! No carrier is made at a *nested* boundary. A slot declared at an application whose pins name
//! the outer signature's parameters is re-viewed against it read under the outer view's bindings,
//! so the nested view's identities are the outer carriers, arriving through the declared type
//! rather than being made again. [`fitted`] and [`build`] are the one fit and the one body both
//! the outer ascription and the nested case go through.
//!
//! A `:|` over a body-born module whose content no demand has computed computes it first
//! ([`fill_at_view`]), from the digest memos of the slots its captures were read from where the
//! load placed them, so a value two views reach through two modules is digested once.
//!
//! [`opaque_view_types`] is the half of an ascription computed from types alone: the bindings
//! either side and the view's signature. The load calls it at a `:|` site over a content tree it
//! sees all of, so the root holds each type the run's view would otherwise intern;
//! [`opaque_view`] computes it beside what *fits* solves, which keys the view's content tree, from
//! one fit. [`nested_types`] is the same half of a nested re-view, which the load lays wherever it
//! knows what a nested module carries (`dispatch/walk/views.rs`).

use crate::knot::{KReading, KValue, Knotted, composed};
use crate::memory::{BumpAllocator, BumpVec, Writer};
use crate::scope::ViewSlots;
use crate::symbols::{BinderSymbol, TypeSymbol, ValueSymbol};
use crate::type_lattice::{
    ContentKey, FitsFailure, KType, Members, SchemaDraft, SigSchema, TypeNode,
    member as bound_member, substitute_parameters,
};
use crate::values::{Chain, ContentDigest, Digests, TypeValue, Value, fit_solution};

use super::coerce::{Coercion, CoercionRefused, Substitution, coerce};
use super::{Born, Module, ModuleContent, Operator, layout};

/// Which operator a program ascribes by: `:!` retypes the value to the type, `:|` views a module
/// through a signature. Only `:|` comes through this door.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ascription {
    /// `:|` — each unpinned head parameter is hidden behind a carrier, and every member is born coerced
    /// to it.
    Opaque,
    /// `:!` — a retype, which re-stamps a module at the application it fits.
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
pub fn takes(types: &Chain<'_, '_, '_, '_>, ascribed: KType) -> bool {
    matches!(
        types.node(ascribed),
        TypeNode::Signature { .. } | TypeNode::SignatureApply { .. }
    )
}

/// Compute the content of `source`, the body-born module a `:|` in `view` reads bare, where no
/// demand has yet: each capture its content composes that the load read from a slot is taken from
/// that slot's memo in `slots`, and a memo still empty is filled from the capture, which holds the
/// value its slot does. Any other capture is digested through one demand opened for them all.
pub fn fill_at_view<'graph, 'cell>(
    source: KValue<'graph, 'cell>,
    slots: ViewSlots<'_>,
    view: &KReading<'_, 'graph, 'cell>,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
) {
    let Some(module) = source.knot_member().and_then(Knotted::module) else {
        return;
    };
    let Some((body, closure)) = module.pending() else {
        return;
    };
    // The memos are placed for the captures of the body the load read; any other module's
    // content is left to the view's own walk.
    if body.segment != slots.body {
        return;
    }
    let mut memo = None;
    let captures = composed(body, closure.links()).map(|(index, link)| {
        let cell = (slots.captures.iter())
            .find(|(each, _)| *each as usize == index)
            .and_then(|(_, at)| view.digest_memo(*at));
        if let Some(digest) = cell.and_then(|cell| cell.get()) {
            return (index, digest);
        }
        let digest = link.digest(memo.get_or_insert_with(|| Digests::new(types, scratch)));
        if let Some(cell) = cell {
            cell.set(Some(digest));
        }
        (index, digest)
    });
    let content = ModuleContent::body(body.code, captures);
    #[cfg(test)]
    if CHECK_SLOT_MEMOS.get() {
        let mut fresh = Digests::new(types, scratch);
        let walked =
            composed(body, closure.links()).map(|(index, link)| (index, link.digest(&mut fresh)));
        assert_eq!(
            content,
            ModuleContent::body(body.code, walked),
            "a slot's memo holds the digest of the value its capture holds"
        );
    }
    module.fill(content);
}

#[cfg(test)]
thread_local! {
    /// Whether [`fill_at_view`] checks the content it builds from memos against a fresh walk of
    /// the module's captures: the memo law.
    pub static CHECK_SLOT_MEMOS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// `source` seen as `signature` under `:|`, as a module of its own. The source's signature is the
/// type it carries — a header's application, or a bare module's own — and the view is keyed on
/// its bare member, so two sources over one module that fit alike build one view. A refusal writes
/// nothing but what a partial coercion walk had already laid down, which nothing names.
pub fn ascribe<'graph, 'tier: 'x, 'cell, 'x>(
    writer: Writer<'cell, '_>,
    source: KValue<'graph, 'cell>,
    signature: KType,
    types: &Chain<'tier, 'cell, '_, '_>,
    scratch: BumpAllocator<'x>,
) -> Result<Knotted<'graph, 'cell>, Unascribable<'tier, 'x>> {
    let member = (source.knot_member())
        .filter(|member| member.module().is_some())
        .ok_or(Unascribable::NotAModule)?;
    let held = (source.ktype().as_type()).expect("a module's type is a signature type");
    // A module's digest is its knot's over its content: a walk of its captures where no demand
    // has computed it yet, none after.
    let bare = Value::Knotted(member).digest(types, scratch);
    let (sig, from, pins) = fitted(held, signature, types, scratch)?;
    let interned = Substitution::interned(from, types, scratch);
    let (viewed, content) = opaque(
        Some(held),
        sig,
        (from, interned),
        &pins,
        signature,
        bare,
        types,
        scratch,
    );
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
    /// What the view binds them to: a carrier for each unpinned one, each pinned one at its pin.
    pub to: Substitution<'run>,
    /// The view's own signature.
    pub view: KType,
}

/// What a nested module of signature `held` binds when re-viewed at a slot whose application
/// reads `src` under the source's bindings and `dst` under the view's: *fits* solves the source's
/// side against `src`, and the view's side takes each parameter `dst` pins, the rest kept as
/// solved. The coercion walk and the load's lay of a view both compute it here.
pub(crate) fn nested_types<'run: 'x, 'x>(
    held: KType,
    src: KType,
    dst: KType,
    types: &Chain<'run, '_, '_, '_>,
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

/// What an opaque view at `signature` of a module whose signature is `held`, where known, binds
/// over a source whose bare member's digest is `source`, computed from types alone, as the load
/// computes it over a source it sees all of: each carrier keyed on the view's content and each
/// pinned parameter at its pin. The very handles [`ascribe`] lays down where the source fits.
/// Without `held`, only a view pinning every parameter, whose fit solves its pins. `None` where
/// the view door would refuse, or the load cannot tell what the fit solves.
pub fn opaque_view_types<'run>(
    held: Option<KType>,
    signature: KType,
    source: ContentDigest,
    types: &Chain<'run, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
) -> Option<ViewTypes<'run>> {
    opaque_view(held, signature, Some(source), types, scratch)?.1
}

/// What a view at `signature` of a module whose signature is `held`, where known, keys its content
/// on — what *fits* solves, interned — beside, where its source's bare member's digest is known,
/// what it binds ([`opaque_view_types`]): one fit for both. `None` where the view door would
/// refuse, or the load cannot tell what the fit solves.
pub fn opaque_view<'run>(
    held: Option<KType>,
    signature: KType,
    source: Option<ContentDigest>,
    types: &Chain<'run, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
) -> Option<(KType, Option<ViewTypes<'run>>)> {
    let (sig, from, pins) = known_fit(held, signature, types, scratch)?;
    let interned = Substitution::interned(from, types, scratch);
    let viewed = source.map(|source| {
        opaque(
            held,
            sig,
            (from, interned),
            &pins,
            signature,
            source,
            types,
            scratch,
        )
        .0
    });
    Some((interned.handle, viewed))
}

/// [`fitted`] where `held` is known; without it, a signature pinning every parameter, whose fit
/// solves its pins.
#[allow(clippy::type_complexity)]
fn known_fit<'run: 'x, 'x>(
    held: Option<KType>,
    signature: KType,
    types: &Chain<'run, '_, '_, '_>,
    scratch: BumpAllocator<'x>,
) -> Option<(
    SigSchema<'run>,
    Members<'x, TypeSymbol, KType>,
    BumpVec<'x, (BinderSymbol, KType)>,
)> {
    if let Some(held) = held {
        return fitted(held, signature, types, scratch).ok();
    }
    let (sig, _, pins) = applied(signature, types, scratch).ok()?;
    let pinned = |name: TypeSymbol| (pins.iter()).any(|(pin, _)| *pin == BinderSymbol::Type(name));
    if !sig.parameters.iter().all(|(name, _)| pinned(*name)) {
        return None;
    }
    let from = Members::from_pairs(
        scratch,
        pins.iter().filter_map(|(name, pin)| match name {
            BinderSymbol::Type(name) => Some((*name, *pin)),
            _ => None,
        }),
    );
    Some((sig, from, pins))
}

/// The opaque view at `signature`, whose schema is `sig` and pins `pins`, of a module whose fit
/// solved `from`, beside it interned, over a bare member whose digest is `source`, beside its
/// content.
#[allow(clippy::too_many_arguments)]
fn opaque<'run>(
    held: Option<KType>,
    sig: SigSchema<'run>,
    (from, interned): (Members<'_, TypeSymbol, KType>, Substitution<'run>),
    pins: &[(BinderSymbol, KType)],
    signature: KType,
    source: ContentDigest,
    types: &Chain<'run, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
) -> (ViewTypes<'run>, ModuleContent) {
    let content = ModuleContent::view(Operator::Opaque, signature, source, interned.handle);
    // A carrier is keyed on the view's content. Two views of equal content share one, and the
    // parameter's name keeps two of one view apart.
    let to = carriers(&sig, from, pins, content.carrier_key(), types, scratch);
    let viewed = ViewTypes {
        held,
        sig,
        from: held.map(|_| interned),
        to: Substitution::interned(to, types, scratch),
        view: view_signature(&sig, to, types, scratch),
    };
    (viewed, content)
}

/// The ascribed signature's schema, what a module of signature `held` binds its head parameters
/// to under *fits*, and the application's pins. Refused where `signature` names no one application
/// of a signature, or `held` does not fit it.
#[allow(clippy::type_complexity)]
pub(super) fn fitted<'run: 'x, 'x>(
    held: KType,
    signature: KType,
    types: &Chain<'run, '_, '_, '_>,
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
        fit_solution(types, scratch, held, declared, &pins).map_err(Unascribable::Unsatisfied)?
    };
    Ok((sig, from, pins))
}

/// The ascribed signature's schema, the signature it applies, and the application's pins. Refused
/// where `signature` names no one application of a signature.
#[allow(clippy::type_complexity)]
fn applied<'run: 'x, 'x>(
    signature: KType,
    types: &Chain<'run, '_, '_, '_>,
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

/// Build the view's member run in layout order and lay the node down, its content `content`. `from`
/// and `to` are what the source and the view bind the signature's head parameters to, interned.
///
/// Two callers: an ascription, and a nested signature slot inside one
/// ([`coerce`](super::coerce::coerce)), which passes the enclosing substitutions unchanged —
/// a nested boundary makes no carrier of its own. Satisfaction is the caller's: an ascription checks
/// it outright, and a nested slot was checked when the enclosing one was.
#[allow(clippy::too_many_arguments)]
pub(super) fn build<'graph, 'tier, 'cell, 'x>(
    writer: Writer<'cell, '_>,
    source: KValue<'graph, 'cell>,
    sig: SigSchema<'tier>,
    view: KType,
    from: Substitution<'tier>,
    to: Substitution<'tier>,
    content: ModuleContent,
    types: &Chain<'tier, 'cell, '_, '_>,
    scratch: BumpAllocator<'x>,
) -> Result<Knotted<'graph, 'cell>, Unascribable<'tier, 'x>> {
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
        let held = layout::member(writer, source, BinderSymbol::Value(name), types, scratch)
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
    // Then, per keyworded member, the source's overloads covering it read under `from` — the pick a
    // header makes — each behind a barrier where the view reads the member otherwise, and retyped
    // to the member's shape the view shows.
    for declared in sig.keyworded.iter().copied() {
        let key = layout::key_of(declared, types);
        let offered = layout::functions_at(writer, source, key, types, scratch);
        let shown = cx.view_side(declared);
        for function in layout::covering(&offered, cx.source_side(declared), types, scratch) {
            let member = coerce(&cx, function, declared)
                .expect("a registration member is a function, which a barrier takes");
            members.push(layout::retyped_at_shape(
                writer, member, shown, types, scratch,
            ));
        }
    }
    Ok(Knotted::of(
        Module::tie(writer, view, &members, Born::View(content), types),
        0,
    ))
}

/// The view's bindings under `:|`: a carrier per head parameter of `sig` that `pins` leaves
/// unpinned — a [`Carrier`](TypeNode::Carrier) keyed on `key`, under the parameter's name, meeting
/// the bound the declaration gives it — and each pinned one at what `from` holds for it, its pin.
fn carriers<'x>(
    sig: &SigSchema<'_>,
    from: Members<'_, TypeSymbol, KType>,
    pins: &[(BinderSymbol, KType)],
    key: ContentKey,
    types: &Chain<'_, '_, '_, '_>,
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
    types: &Chain<'_, '_, '_, '_>,
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
