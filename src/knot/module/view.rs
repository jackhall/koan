//! The view door: what `m :| Sig` builds. (`m :! Sig` is a retype, which re-stamps the module at
//! the application it fits under a header, and never comes here.)
//!
//! A view is a module of its own — the same node, through the same constructor — holding only the
//! members its signature names, at the types that signature declares. The ascribed type is one
//! application of a declared signature; a meet of several is refused. The source must *fit* it
//! ([`fit_solution`]), its signature the type it carries: a header's application, or a bare
//! module's own.
//!
//! Every module is a view. A body-born module, and a module a barrier re-views inward, is a
//! transparent view of itself, its own **underlying**; an opaque view keeps a reference to the
//! module it is built over and a [`ViewRecord`] of how, each carrier its types show beside what it
//! hides over that module among it. A view is built over its source's underlying, so a view of a
//! view is a view of that module ([`exposure`]): the source as written decides only admission,
//! and what *fits* solves over the underlying's signature at the application read back through
//! the carriers it holds, in its signature and its pins alike, `from*`, keys the view. A view of a
//! view must hide again every part its source seals ([`hides_again`]), or it is refused: built
//! over the underlying it would unseal them. Where the content is the source's, the door returns
//! the source.
//!
//! Each unpinned parameter is hidden behind a **carrier keyed on content**: a
//! [`Carrier`](TypeNode::Carrier) keyed on the view's own [content](ModuleContent::carrier_key) —
//! its application over its underlying's digest and `from*` — so two views that show one exposure
//! of one module share their carriers, and two of other content never unify, however often either
//! runs. Every member is then born [coerced](super::coerce) to the carriers. A pinned parameter
//! keeps its pin.
//!
//! A view carries a keyworded member too: past its named members, the overloads the underlying
//! offers at the member's key that cover the member read under `from*`, narrowed to the most
//! specific ([`layout::covering`], the pick a header makes), each
//! [coerced](super::coerce::coerce) as any member is — behind a barrier where the view reads the
//! member otherwise — and retyped to the member's shape the view shows. Over a body-born module the
//! load sees all of, it recorded that pick by registration position ([`picks`]), and the build
//! takes it.
//!
//! A nested module member is a view too ([`coerce`](super::coerce::coerce)): outward an opaque
//! view of the underlying's member through the same [`exposure`] and [`build`], inward a
//! transparent re-view ([`nested_types`]).
//!
//! A `:|` over a body-born module whose content no demand has computed computes it first
//! ([`fill_at_view`]), from the digest memos of the slots its captures were read from where the
//! load placed them, so a value two views reach through two modules is digested once.
//!
//! [`exposure`] is computed from types alone, over an [`Operand`] — what the source carries, its
//! bare member's digest, and that member as a view ([`Bare`]) — so the load computes it too, at a
//! `:|` site over a content tree it sees (`dispatch/walk/views.rs`), and the root holds each type
//! the run's view would otherwise intern. A slot that pins every parameter needs no operand at all
//! ([`pinned_view`]): *fits* solves a fully pinned application to its pins, so the load lays a
//! nested view of a module it knows only by its type.

use crate::knot::{KReading, KValue, Knotted, composed};
use crate::memory::{BumpAllocator, BumpVec, Writer};
use crate::scope::{Bare, Pick, Revealed, Sealing, ViewRecord, ViewSlots};
use crate::symbols::{BinderSymbol, TypeSymbol, ValueSymbol};
use crate::type_lattice::{
    ContentKey, DeclaredType, DispatchTokenElement, FitsFailure, KType, Members, Parametric,
    SchemaDraft, SigSchema, TypeNode, member as bound_member, substitute_carriers,
    substitute_parameters,
};
use crate::values::{Chain, ContentDigest, Digests, TypeValue, Value, fit_solution};

use super::coerce::{Coercion, Hidden, Substitution, coerce};
use super::{Born, Module, ModuleContent, Operator, layout};
use crate::scope::CoercionRefused;

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
    /// A view of a view shows `member`, a part its source hides.
    Reveals { member: Revealed },
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
/// type it carries — a header's application, or a bare module's own — and decides only whether
/// the view is admitted: the view is built over the source's underlying and keyed on it
/// ([`exposure`]), so two sources that show one exposure of one module build one view, and a view
/// whose content is its source's returns the source. `picks` are what the load recorded for a
/// body-born source it saw all of ([`picks`]). A refusal writes nothing but what a partial
/// coercion walk had already laid down, which nothing names.
pub fn ascribe<'graph, 'tier: 'x, 'cell, 'x>(
    writer: Writer<'cell, '_>,
    source: KValue<'graph, 'cell>,
    signature: KType,
    picks: Option<&[Pick<'_>]>,
    types: &Chain<'tier, 'cell, '_, '_>,
    scratch: BumpAllocator<'x>,
) -> Result<Knotted<'graph, 'cell>, Unascribable<'tier, 'x>> {
    let member = (source.knot_member())
        .filter(|member| member.module().is_some())
        .ok_or(Unascribable::NotAModule)?;
    let held = (source.ktype().as_type()).expect("a module's type is a signature type");
    let exposure = exposure(
        operand_of(Some(held), member, types, scratch),
        signature,
        signature,
        &[],
        types,
        scratch,
    )?;
    built_over(writer, member, &exposure, picks, false, types, scratch)
}

/// The view `exposure` describes over `member`, the bare module it was computed for: `member`
/// itself where the view is it, else a module built over its underlying.
pub(super) fn built_over<'graph, 'tier, 'cell, 'x>(
    writer: Writer<'cell, '_>,
    member: Knotted<'graph, 'cell>,
    exposure: &Exposure<'tier, '_>,
    picks: Option<&[Pick<'_>]>,
    inward: bool,
    types: &Chain<'tier, 'cell, '_, '_>,
    scratch: BumpAllocator<'x>,
) -> Result<Knotted<'graph, 'cell>, Unascribable<'tier, 'x>> {
    let (Some(from), Some(built)) = (exposure.from, exposure.built) else {
        unreachable!("a held signature over a module the run holds is a closed exposure")
    };
    if built.returns_source {
        return Ok(member);
    }
    // A transparent view of itself is its own underlying.
    let underlying = (member.module())
        .and_then(Module::underlying)
        .unwrap_or(member);
    let hidden = Hidden {
        carriers: built.record.hidden,
        on_from: true,
    };
    build(
        writer,
        Value::Knotted(underlying),
        exposure.sig,
        from,
        built,
        Some(underlying),
        picks,
        inward,
        hidden,
        types,
        scratch,
    )
}

/// What a `:|` reads of its operand: the type it carries, its bare member's digest, and that
/// member as a view, each where known.
#[derive(Clone, Copy)]
pub struct Operand<'a> {
    pub held: Option<KType>,
    pub digest: Option<ContentDigest>,
    pub bare: Option<Bare<'a>>,
}

/// The operand a run value's bare member `member`, carried at `held`, is.
pub fn operand_of<'cell>(
    held: Option<KType>,
    member: Knotted<'_, 'cell>,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
) -> Operand<'cell> {
    Operand {
        held,
        digest: Some(Value::Knotted(member).digest(types, scratch)),
        bare: Some(bare_of(member, types, scratch)),
    }
}

/// `member`, a module, as a view over it reads it: its own signature, and for an opaque view its
/// record and its underlying's signature and digest.
pub fn bare_of<'cell>(
    member: Knotted<'_, 'cell>,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
) -> Bare<'cell> {
    let module = member.module().expect("a module");
    let sealing = (module.view_record())
        .zip(module.underlying())
        .map(|(record, underlying)| Sealing {
            record,
            underlying_signature: underlying.module().expect("an underlying module").ktype(),
            underlying: Value::Knotted(underlying).digest(types, scratch),
        });
    Bare {
        signature: Some(module.ktype()),
        sealing,
    }
}

/// What `:|` at a signature builds, computed from types alone: the schema, and, where the operand
/// tells, `from*` — what *fits* solves each head parameter to over the underlying — and what the
/// view is built with.
#[derive(Clone, Copy)]
pub struct Exposure<'run, 'h> {
    /// The ascribed signature's schema.
    pub sig: SigSchema<'run>,
    /// `from*`: what the underlying binds the head parameters to. `None` where the view is open.
    pub from: Option<Substitution<'run>>,
    /// What the view is built with, where `from*` and the underlying's digest are known.
    pub built: Option<Built<'run, 'h>>,
}

/// What a view is built with, once its `from*` and its underlying's digest are known.
#[derive(Clone, Copy)]
pub struct Built<'run, 'h> {
    /// What the view binds the head parameters to: a carrier for each unpinned one, each pinned
    /// one at its pin.
    pub to: Substitution<'run>,
    /// The view's own signature.
    pub view: KType,
    /// What the view is: its application over its underlying's digest and `from*`.
    pub content: ModuleContent,
    /// What the view keeps of how it was built.
    pub record: ViewRecord<'h>,
    /// Whether the view is its source: an opaque source of this application and `from*`.
    pub returns_source: bool,
}

/// What `:|` at `signature` over `operand` builds, admitted at `admitted_at`, from types alone.
/// For `:|` the two are the ascribed application; for a nested module member `signature` is its
/// slot's application on the view's side and `admitted_at` on the underlying's, and `context` the
/// enclosing view's carriers beside what each hides, which the slot's pins may name.
///
/// The operand's carried type admits it. Where its bare member is an opaque view, every part that
/// view seals must sit in the new view at a carrier ([`hides_again`]), and the new view is built
/// over that view's underlying; a transparent source is its own underlying. `from*` is what *fits*
/// solves over the underlying's signature at the application read through the carriers it holds
/// onto the underlying, inside its signature too, so the source as written decides only
/// admission. Open — no `from*` — where the load does not know the carried type, the bare member
/// as a view, or, over a source known to seal nothing, a fit that solves only its pins.
pub fn exposure<'run: 'x, 'x>(
    operand: Operand<'_>,
    signature: KType,
    admitted_at: KType,
    context: &[(KType, KType)],
    types: &Chain<'run, '_, '_, '_>,
    scratch: BumpAllocator<'x>,
) -> Result<Exposure<'run, 'x>, Unascribable<'run, 'x>> {
    let (sig, _, pins) = applied(signature, types, scratch)?;
    let open = Exposure {
        sig,
        from: None,
        built: None,
    };
    let admitted = match operand.held {
        Some(held) => Some(fitted(held, admitted_at, types, scratch)?.1),
        None => None,
    };
    let Some(bare) = operand.bare else {
        return Ok(open);
    };
    // The carriers a pin may read back: the enclosing view's, and the source's.
    let mut known = BumpVec::new_in(scratch);
    known.extend_from_slice(context);
    let underlying = match bare.sealing {
        Some(sealing) => {
            let (Some(offered), Some(own)) = (admitted, bare.signature) else {
                return Ok(open);
            };
            let offered = match operand.held == Some(own) {
                true => offered,
                false => fitted(own, admitted_at, types, scratch)?.1,
            };
            hides_again(&sig, &pins, offered, sealing.record, types, scratch)
                .map_err(|member| Unascribable::Reveals { member })?;
            known.extend_from_slice(sealing.record.hidden);
            (Some(sealing.underlying_signature), Some(sealing.underlying))
        }
        None => (bare.signature, operand.digest),
    };
    let known = Hidden {
        carriers: known.leak(),
        on_from: true,
    };
    let read_back = |kt: KType| substitute_carriers(types, scratch, kt, &mut |c| known.of(c));
    let (_, _, admitted_pins) = applied(admitted_at, types, scratch)?;
    let from = match (admitted, underlying.0) {
        // Over the underlying, the application read through the carriers it holds, inside its
        // signature and its pins alike. *Fits* is transitive, so a fit the operand admitted holds
        // there.
        (Some(_), Some(signature)) => fitted(signature, read_back(admitted_at), types, scratch)?.1,
        // Over a source known to seal nothing whose type the load does not know, only a view
        // pinning every parameter, whose fit solves its pins.
        (None, _) if bare.sealing.is_none() => match pinned_all(&sig, &admitted_pins, scratch) {
            Some(from) => from,
            None => return Ok(open),
        },
        _ => return Ok(open),
    };
    let from = Substitution::interned(from, types, scratch);
    let Some(underlying) = underlying.1 else {
        return Ok(Exposure {
            sig,
            from: Some(from),
            built: None,
        });
    };
    let content = ModuleContent::view(Operator::Opaque, signature, underlying, from.handle);
    // A carrier is keyed on the view's content. Two views of equal content share one, and the
    // parameter's name keeps two of one view apart.
    let to = carriers(&sig, &pins, content.carrier_key(), types, scratch);
    let view = view_signature(&sig, to, types, scratch);
    let hidden = hidden_by(signature, to, from.bindings, known, types, scratch);
    let to = Substitution::interned(to, types, scratch);
    let record = ViewRecord {
        application: signature,
        from: from.handle,
        to: to.handle,
        hidden,
    };
    let returns_source = (bare.sealing).is_some_and(|sealing| {
        sealing.record.application == signature && sealing.record.from == from.handle
    });
    Ok(Exposure {
        sig,
        from: Some(from),
        built: Some(Built {
            to,
            view,
            content,
            record,
            returns_source,
        }),
    })
}

/// What a nested view at `signature`, admitted at `admitted_at`, binds where both pin every head
/// parameter: its schema, `from*` (the admitted pins, which *fits* solves to over any module), its
/// `to` (the view-side pins), its own signature and the carriers `to` shows, each beside what
/// `context` says it hides — all from types alone, whatever module the view is built over, so the
/// load lays them for a nested module it knows by its type alone. `None` where a parameter is
/// left unpinned, whose carrier is keyed on the view's content.
pub fn pinned_view<'run: 'x, 'x>(
    signature: KType,
    admitted_at: KType,
    context: &[(KType, KType)],
    types: &Chain<'run, '_, '_, '_>,
    scratch: BumpAllocator<'x>,
) -> Option<PinnedView<'run, 'x>> {
    let (sig, _, pins) = applied(signature, types, scratch).ok()?;
    let (_, _, admitted) = applied(admitted_at, types, scratch).ok()?;
    let (to, from) = (
        pinned_all(&sig, &pins, scratch)?,
        pinned_all(&sig, &admitted, scratch)?,
    );
    let view = view_signature(&sig, to, types, scratch);
    let from = Substitution::interned(from, types, scratch);
    let known = Hidden {
        carriers: context,
        on_from: true,
    };
    let hidden = hidden_by(signature, to, from.bindings, known, types, scratch);
    Some(PinnedView {
        sig,
        from,
        to: Substitution::interned(to, types, scratch),
        view,
        hidden,
    })
}

/// What [`pinned_view`] computes: a nested view's schema, `from*`, `to`, own signature, and the
/// carriers `to` shows beside what each hides.
pub struct PinnedView<'run, 'h> {
    pub sig: SigSchema<'run>,
    pub from: Substitution<'run>,
    pub to: Substitution<'run>,
    pub view: KType,
    pub hidden: &'h [(KType, KType)],
}

/// Each head parameter of `sig` at its pin among `pins`, where they pin every one.
fn pinned_all<'x>(
    sig: &SigSchema<'_>,
    pins: &[(BinderSymbol, KType)],
    scratch: BumpAllocator<'x>,
) -> Option<Members<'x, TypeSymbol, KType>> {
    let pinned = |name: TypeSymbol| (pins.iter()).any(|(pin, _)| *pin == BinderSymbol::Type(name));
    if !sig.parameters.iter().all(|(name, _)| pinned(*name)) {
        return None;
    }
    Some(Members::from_pairs(
        scratch,
        pins.iter().filter_map(|(name, pin)| match name {
            BinderSymbol::Type(name) => Some((*name, *pin)),
            _ => None,
        }),
    ))
}

/// The carriers a view at `application`, binding its head parameters to `to`, shows, each beside
/// what it hides over its underlying: a carrier the view made, what `from*` binds its parameter
/// to; one the application names — in a pin or in its signature — what `known` says it hides. A
/// carrier neither tells is one the underlying's own types hold.
fn hidden_by<'x>(
    application: KType,
    to: Members<'_, TypeSymbol, KType>,
    from: Members<'_, TypeSymbol>,
    known: Hidden<'_>,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'x>,
) -> &'x [(KType, KType)] {
    let mut hidden: BumpVec<'x, (KType, KType)> = BumpVec::new_in(scratch);
    for (name, bound) in to.iter() {
        let made = (bound_member(from, *name))
            .and_then(|solved| types.concrete(solved))
            .filter(|solved| types.is_carrier(*bound) && solved != bound);
        if let Some(solved) = made {
            hidden.push((*bound, solved));
        }
    }
    // A pin binds its parameter in `to`, so this reaches every known carrier `to` holds too.
    substitute_carriers(types, scratch, application, &mut |carrier| {
        if let Some(hides) = known.of(carrier) {
            hidden.push((carrier, hides));
        }
        None
    });
    hidden.sort_unstable_by_key(|(carrier, _)| *carrier);
    hidden.dedup();
    hidden.leak()
}

/// What a module of signature `held` binds when re-viewed transparently, crossing a barrier inward,
/// at a slot whose application reads `src` under the barrier's outer side and `dst` under the
/// function's: *fits* solves the source's side against `src`, and the re-view, at `dst`'s
/// signature, takes each parameter `dst` pins, the rest kept as solved. Its content, over the
/// module's digest `bare`, is tagged apart from an opaque view's; its record is kept by no module,
/// since the re-view is a transparent view of itself.
pub(crate) fn nested_types<'run: 'x, 'x>(
    held: KType,
    src: KType,
    dst: KType,
    bare: ContentDigest,
    types: &Chain<'run, '_, '_, '_>,
    scratch: BumpAllocator<'x>,
) -> Result<Exposure<'run, 'x>, Unascribable<'run, 'x>> {
    // An application's two sides share one declared signature; a bare signature's `dst` is `src`
    // read through the carriers it names, so the re-view's own type is laid from `dst`'s schema,
    // while its members are built over `src`'s and unsealed through the barrier's carriers.
    let (shown, _, pins) = applied(dst, types, scratch)?;
    let (sig, from, _) = fitted(held, src, types, scratch)?;
    let to = Members::from_pairs(
        scratch,
        from.iter().map(|(name, solved)| {
            let pinned = (pins.iter()).find(|(pin, _)| *pin == BinderSymbol::Type(*name));
            (*name, pinned.map_or(*solved, |(_, pin)| *pin))
        }),
    );
    let view = view_signature(&shown, to, types, scratch);
    let (from, to) = (
        Substitution::interned(from, types, scratch),
        Substitution::interned(to, types, scratch),
    );
    Ok(Exposure {
        sig,
        from: Some(from),
        built: Some(Built {
            to,
            view,
            content: ModuleContent::view(Operator::Reviewed, dst, bare, from.handle),
            record: ViewRecord {
                application: dst,
                from: from.handle,
                to: to.handle,
                hidden: &[],
            },
            returns_source: false,
        }),
    })
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

/// Whether a view at `sig`, pinned at `pins`, of the opaque view `record` describes hides again
/// every part that view seals: each sits, in the new view, at a carrier — a head parameter it
/// leaves unpinned, or one pinned at a carrier — or at a nested module's parameter its slot
/// leaves unpinned, which the nested view hides. A pin is read as the type it is, so a sealed part
/// may sit at a carrier deep in it, and a source's parameter as what the source binds it to.
/// `offered` is what *fits* solved the new signature's parameters to over the source. A question
/// about types alone, walked on the two declared types side by side as the coercion walk recurses
/// on one, the source's underlying side read through its carriers; `Err` names the first member,
/// in the new signature's order, it fails at.
fn hides_again(
    sig: &SigSchema<'_>,
    pins: &[(BinderSymbol, KType)],
    offered: Members<'_, TypeSymbol, KType>,
    record: ViewRecord,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'_>,
) -> Result<(), Revealed> {
    let (old, _, _) = applied(record.application, types, scratch)
        .unwrap_or_else(|_| unreachable!("a view's record holds the application it was built at"));
    let mut pinned = BumpVec::new_in(scratch);
    pinned.extend(pins.iter().filter_map(|(name, pin)| match name {
        BinderSymbol::Type(name) => Some((*name, Parametric::from(*pin))),
        _ => None,
    }));
    let walk = Rehiding {
        types,
        scratch,
        offered,
        to: Substitution::of(record.to, types).bindings,
        from: Substitution::of(record.from, types).bindings,
        hidden: record.hidden,
    };
    let new = Level {
        parameters: sig.parameters,
        pins: &pinned,
        up: None,
    };
    let source = Level {
        parameters: old.parameters,
        pins: &[],
        up: None,
    };
    walk.members(sig, &new, &old, &source)
}

/// One level of signature nesting a declared type is read at: its signature's head parameters,
/// each pinned over the level above or left unpinned. The outermost level reads its parameters
/// through the walk's own bindings.
struct Level<'l, 'run> {
    parameters: Members<'run, TypeSymbol>,
    pins: &'l [(TypeSymbol, Parametric)],
    up: Option<&'l Level<'l, 'run>>,
}

impl Level<'_, '_> {
    fn pin(&self, name: TypeSymbol) -> Option<Parametric> {
        (self.pins.iter()).find_map(|(pinned, pin)| (*pinned == name).then_some(*pin))
    }
}

/// Which reading of a declared type: the new view's over the source, or the source's view side or
/// its underlying's side.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Reading {
    New,
    SourceView,
    SourceUnderlying,
}

/// The [`hides_again`] walk: the new signature's parameters as *fits* solved them over the source,
/// and the source's two sides.
struct Rehiding<'a, 'run, 'cell, 'reg, 'h, 'x> {
    types: &'a Chain<'run, 'cell, 'reg, 'h>,
    scratch: BumpAllocator<'x>,
    offered: Members<'a, TypeSymbol, KType>,
    to: Members<'run, TypeSymbol>,
    from: Members<'run, TypeSymbol>,
    /// The source's carriers, each beside what it hides over the underlying.
    hidden: &'a [(KType, KType)],
}

impl<'run, 'x> Rehiding<'_, 'run, '_, '_, '_, 'x> {
    /// Each value slot and keyworded member of `new`, read at `nl`, against the source's members
    /// in `old`, read at `ol`. A member `new` drops checks nothing.
    fn members(
        &self,
        new: &SigSchema<'_>,
        nl: &Level<'_, '_>,
        old: &SigSchema<'_>,
        ol: &Level<'_, '_>,
    ) -> Result<(), Revealed> {
        for (name, declared) in new.value_slots.iter().copied() {
            let rehidden = bound_member(old.value_slots, name)
                .is_some_and(|held| self.rehides(declared, nl, held, ol));
            if !rehidden {
                return Err(Revealed::Slot(name));
            }
        }
        if new.keyworded.is_empty() {
            return Ok(());
        }
        // A keyworded member against each of the source's it covers, as the build picks them.
        let mut registered = BumpVec::with_capacity_in(old.keyworded.len(), self.scratch);
        for held in old.keyworded.iter().copied() {
            let read = self.read(held, ol, Reading::SourceView);
            registered.push(
                self.types
                    .concrete_declared(read)
                    .ok_or(Revealed::Keyworded(layout::key_of(held, self.types)))?,
            );
        }
        for declared in new.keyworded.iter().copied() {
            let refused = Revealed::Keyworded(layout::key_of(declared, self.types));
            let shown = self.read(declared, nl, Reading::New);
            let shown = self.types.concrete_declared(shown).ok_or(refused)?;
            for at in layout::offered_at(&registered, shown, self.types, self.scratch) {
                if !self.rehides(declared, nl, old.keyworded[at as usize], ol) {
                    return Err(refused);
                }
            }
        }
        Ok(())
    }

    /// `declared`, read at `level` through every level above it: a nested level's parameter at
    /// its pin, an unpinned one at a stand-in that differs between the source's two sides, and
    /// the outermost level's under the reading's own bindings.
    fn read(
        &self,
        declared: DeclaredType<Parametric>,
        level: &Level<'_, '_>,
        reading: Reading,
    ) -> DeclaredType<Parametric> {
        let (types, scratch) = (self.types, self.scratch);
        let unpinned = match reading {
            Reading::New | Reading::SourceView => KType::ANY,
            Reading::SourceUnderlying => KType::NEVER,
        };
        let mut read = declared;
        let mut at = level;
        while let Some(up) = at.up {
            let bindings = Members::from_pairs(
                scratch,
                (at.parameters.iter())
                    .map(|(name, _)| (*name, at.pin(*name).unwrap_or(unpinned.into()))),
            );
            read = substitute_parameters(types, scratch, read, bindings);
            at = up;
        }
        match reading {
            Reading::New => substitute_parameters(types, scratch, read, self.offered),
            Reading::SourceView => substitute_parameters(types, scratch, read, self.to),
            Reading::SourceUnderlying => {
                let read = substitute_parameters(types, scratch, read, self.from);
                substitute_carriers(types, scratch, read, &mut |carrier| {
                    crate::scope::hides(self.hidden, carrier)
                })
            }
        }
    }

    /// A signature type's declared schema and its type pins: an application's, or a bare
    /// signature's own schema, pinning nothing. `None` for an application of no signature.
    fn schema_and_pins(
        &self,
        node: TypeNode<'run, Parametric>,
    ) -> Option<(SigSchema<'run>, BumpVec<'x, (TypeSymbol, Parametric)>)> {
        let mut pinned = BumpVec::new_in(self.scratch);
        let schema = match node {
            TypeNode::Signature { schema, .. } => schema,
            TypeNode::SignatureApply { signature, pins } => {
                pinned.extend(pins.iter().filter_map(|(name, pin)| match name {
                    BinderSymbol::Type(name) => Some((name, pin)),
                    _ => None,
                }));
                crate::elaborate::schema_of(signature, self.types)?
            }
            _ => return None,
        };
        Some((schema, pinned))
    }

    /// Whether the source sealed nothing in `old`, read at `ol`: its two sides read alike.
    fn sealed_nothing(&self, old: DeclaredType<Parametric>, ol: &Level<'_, '_>) -> bool {
        self.read(old, ol, Reading::SourceView) == self.read(old, ol, Reading::SourceUnderlying)
    }

    /// Whether `new`, read at `nl`, hides again everything the source seals in `old`, read at
    /// `ol`. A false answer refuses the program, so each arm says exactly that.
    fn rehides(
        &self,
        new: DeclaredType<Parametric>,
        nl: &Level<'_, '_>,
        old: DeclaredType<Parametric>,
        ol: &Level<'_, '_>,
    ) -> bool {
        let types = self.types;
        if self.sealed_nothing(old, ol) {
            return true;
        }
        let node = new.view(types).body;
        if let TypeNode::Parameter { name, .. } = node {
            // Unpinned, it is behind a carrier of the new view or of a nested one; pinned, it is
            // what its pin is, over the level above.
            return match nl.pin(name) {
                None => true,
                Some(pin) => self.rehides(pin.into(), nl.up.unwrap_or(nl), old, ol),
            };
        }
        // A carrier, written in the signature or a pin.
        let carrier = (new.as_type())
            .and_then(|new| types.concrete(new))
            .is_some_and(|new| types.is_carrier(new));
        if carrier {
            return true;
        }
        if let TypeNode::Parameter { name, .. } = old.view(types).body {
            // A part of the source's own type: at the outermost level what the source binds it
            // to, read as a type over the source's carriers; at a nested level its pin, over the
            // level above. One a nested level leaves unpinned a nested view seals whole.
            return match ol.up {
                None => (bound_member(self.to, name))
                    .is_some_and(|bound| self.rehides(new, nl, bound.into(), ol)),
                Some(up) => (ol.pin(name)).is_some_and(|pin| self.rehides(new, nl, pin.into(), up)),
            };
        }
        let each = |new: Parametric, old: Parametric| self.rehides(new.into(), nl, old.into(), ol);
        match (node, old.view(types).body) {
            (TypeNode::List { element: new }, TypeNode::List { element: old }) => each(new, old),
            (TypeNode::Dict { value: new, .. }, TypeNode::Dict { value: old, .. }) => {
                each(new, old)
            }
            (TypeNode::Record { fields: new }, TypeNode::Record { fields: old }) => (new.iter())
                .all(|(name, new)| old.get(name.symbol()).is_some_and(|old| each(new, old))),
            (
                TypeNode::KFunction {
                    params: new,
                    ret: new_ret,
                },
                TypeNode::KFunction {
                    params: old,
                    ret: old_ret,
                },
            ) => {
                (new.iter())
                    .all(|(name, new)| old.get(name.symbol()).is_some_and(|old| each(new, old)))
                    && each(new_ret, old_ret)
            }
            (
                TypeNode::ExpressionShape {
                    elements: new,
                    ret: new_ret,
                    ..
                },
                TypeNode::ExpressionShape {
                    elements: old,
                    ret: old_ret,
                    ..
                },
            ) => {
                let slots =
                    |elements: &mut dyn Iterator<Item = DispatchTokenElement<Parametric>>| {
                        let mut slots = BumpVec::new_in(self.scratch);
                        slots.extend(elements.filter_map(|element| match element {
                            DispatchTokenElement::Slot(slot) => Some(slot),
                            DispatchTokenElement::Keyword(_) => None,
                        }));
                        slots
                    };
                let (new, old) = (slots(&mut new.iter()), slots(&mut old.iter()));
                new.len() == old.len()
                    && (new.iter().zip(old.iter())).all(|(new, old)| each(*new, *old))
                    && each(new_ret, old_ret)
            }
            // A bare signature on either side is the application of it that pins nothing.
            (
                new @ (TypeNode::Signature { .. } | TypeNode::SignatureApply { .. }),
                old @ (TypeNode::Signature { .. } | TypeNode::SignatureApply { .. }),
            ) => {
                let (Some((new, new_pins)), Some((old, old_pins))) =
                    (self.schema_and_pins(new), self.schema_and_pins(old))
                else {
                    return false;
                };
                let inner_new = Level {
                    parameters: new.parameters,
                    pins: &new_pins,
                    up: Some(nl),
                };
                let inner_old = Level {
                    parameters: old.parameters,
                    pins: &old_pins,
                    up: Some(ol),
                };
                self.members(&new, &inner_new, &old, &inner_old).is_ok()
            }
            // Each member the source sealed something in, against the new union's member it reads
            // as over the source: paired by handle, never by position.
            (TypeNode::Union { members: new }, TypeNode::Union { members: old }) => {
                old.iter().all(|old| {
                    let old: DeclaredType<Parametric> = old.into();
                    if self.sealed_nothing(old, ol) {
                        return true;
                    }
                    let shown = self.read(old, ol, Reading::SourceView);
                    (new.iter())
                        .map(DeclaredType::from)
                        .find(|new| self.read(*new, nl, Reading::New) == shown)
                        .is_some_and(|new| self.rehides(new, nl, old, ol))
                })
            }
            // A part the source sealed whole, at the new union's member it reads as over the
            // source, as the coercion walk's union arm takes it.
            (TypeNode::Union { members: new }, _) => {
                let shown = self.read(old, ol, Reading::SourceView);
                (new.iter())
                    .map(DeclaredType::from)
                    .find(|new| self.read(*new, nl, Reading::New) == shown)
                    .is_some_and(|new| self.rehides(new, nl, old, ol))
            }
            _ => false,
        }
    }
}

/// Build the view's member run over `source` in layout order and lay the node down, as `built`
/// describes it. `from` and `to` are what `source` and the view bind the signature's head
/// parameters to, interned. `underlying`, for an opaque view, is the module it is built over,
/// which the view keeps beside its record; `None` for a transparent re-view. `picks`, where the
/// load recorded them, are each keyworded member's covering registrations. `inward` says the view
/// is a re-view crossing a barrier inward, whose nested modules are re-viewed in turn; `hidden`
/// the carriers its members' types read back on the underlying's side.
///
/// Two callers: an ascription and an outward nested slot ([`built_over`]), and an inward nested
/// slot ([`coerce`](super::coerce::coerce)). Satisfaction is the caller's.
#[allow(clippy::too_many_arguments)]
pub(super) fn build<'graph, 'tier, 'cell, 'x>(
    writer: Writer<'cell, '_>,
    source: KValue<'graph, 'cell>,
    sig: SigSchema<'tier>,
    from: Substitution<'tier>,
    built: Built<'tier, '_>,
    underlying: Option<Knotted<'graph, 'cell>>,
    picks: Option<&[Pick<'_>]>,
    inward: bool,
    hidden: Hidden<'_>,
    types: &Chain<'tier, 'cell, '_, '_>,
    scratch: BumpAllocator<'x>,
) -> Result<Knotted<'graph, 'cell>, Unascribable<'tier, 'x>> {
    let cx = Coercion {
        writer,
        types,
        scratch,
        from,
        to: built.to,
        inward,
        hidden,
    };
    let view = built.view;
    let view_schema = crate::elaborate::schema_of(view, types).expect("the view's own signature");
    let mut members: BumpVec<'x, KValue<'graph, 'cell>> = BumpVec::with_capacity_in(
        crate::elaborate::member_count(&view_schema, scratch),
        scratch,
    );
    // Value members first, in the signature's own order — which is layout order, since both
    // schemas' value tables hold the same names symbol-sorted.
    for (name, declared) in sig.value_slots.iter().copied() {
        let held = layout::member(
            writer,
            source,
            (BinderSymbol::Value(name), None),
            types,
            scratch,
        )
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
    for (member, declared) in sig.keyworded.iter().copied().enumerate() {
        let shown = cx.view_side(declared);
        let picked =
            picks.and_then(|picks| picks.iter().find(|pick| usize::from(pick.member) == member));
        let functions = match picked {
            // What the load picked, by position in the source's registration run.
            Some(pick) => {
                let picked = layout::picked_of(source, pick, types, scratch);
                debug_assert!(
                    {
                        let key = layout::key_of(declared, types);
                        let offered =
                            layout::functions_at(writer, source, key, None, types, scratch);
                        let covered =
                            layout::covering(&offered, cx.source_side(declared), types, scratch);
                        layout::same_shapes(&covered, &picked)
                    },
                    "the load's picks are what the build would cover"
                );
                picked
            }
            None => {
                #[cfg(test)]
                layout::COVERED.with(|covered| covered.set(covered.get() + 1));
                let key = layout::key_of(declared, types);
                let offered = layout::functions_at(writer, source, key, None, types, scratch);
                layout::covering(&offered, cx.source_side(declared), types, scratch)
            }
        };
        for function in functions {
            let member = coerce(&cx, function, declared)
                .expect("a registration member is a function, which a barrier takes");
            members.push(layout::retyped_at_shape(
                writer, member, shown, types, scratch,
            ));
        }
    }
    Ok(Knotted::of(
        Module::tie(
            writer,
            view,
            &members,
            Born::View {
                content: built.content,
                view: underlying.map(|underlying| (built.record, underlying)),
            },
            types,
        ),
        0,
    ))
}

/// What a view of `viewed`'s signature, over a body-born module whose registrations register at
/// `registered` in their run's order, carries for each keyworded member: [`build`]'s pick, the
/// registrations covering the member read under `from*`, by position. The load records it where
/// it sees all of the module, and the build takes it in place of its own.
pub fn picks<'x>(
    viewed: &Exposure<'_, '_>,
    registered: &[DeclaredType<KType>],
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'x>,
) -> Option<BumpVec<'x, (u16, BumpVec<'x, u32>)>> {
    let from = viewed.from?;
    let mut picks = BumpVec::with_capacity_in(viewed.sig.keyworded.len(), scratch);
    for (member, declared) in viewed.sig.keyworded.iter().enumerate() {
        let source_side = types.concrete_declared(substitute_parameters(
            types,
            scratch,
            *declared,
            from.bindings,
        ))?;
        let offered = layout::offered_at(registered, source_side, types, scratch);
        picks.push((u16::try_from(member).ok()?, offered));
    }
    Some(picks)
}

/// The view's bindings under `:|`: a carrier per head parameter of `sig` that `pins` leaves
/// unpinned — a [`Carrier`](TypeNode::Carrier) keyed on `key`, under the parameter's name, meeting
/// the bound the declaration gives it — and each pinned one at its pin.
fn carriers<'x>(
    sig: &SigSchema<'_>,
    pins: &[(BinderSymbol, KType)],
    key: ContentKey,
    types: &Chain<'_, '_, '_, '_>,
    scratch: BumpAllocator<'x>,
) -> Members<'x, TypeSymbol, KType> {
    Members::from_pairs(
        scratch,
        sig.parameters.iter().map(|(name, declared)| {
            let pinned = (pins.iter()).find(|(pin, _)| *pin == BinderSymbol::Type(*name));
            let to = match pinned {
                Some((_, pin)) => *pin,
                None => {
                    let bound = types.node(*declared).rigid_bound().unwrap_or(KType::ANY);
                    types.carrier(*name, bound, key)
                }
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
