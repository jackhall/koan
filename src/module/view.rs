//! The view door: what `m :! Sig` and `m :| Sig` build.
//!
//! A view is a module of its own — the same node, through the same constructor — holding only the
//! members its signature names, at the types that signature declares. Both operators narrow; they
//! differ in what the signature's abstract members mean afterwards.
//!
//! Under `:!` they mean what the source binds them to, so the view's members are the source's own
//! words and the view is a relabelling. Under `:|` each one is **minted afresh, once per
//! application**: a rigid variable carrying a nonce nothing else can name, so two ascriptions of
//! one signature over one module produce views whose carriers do not unify. Every member is then
//! born [coerced](super::coerce) to the mints.
//!
//! Nothing is minted at a *nested* boundary. A slot declared at a nested signature is re-viewed
//! against that signature read under the outer view's bindings, so the nested view's abstract
//! identities are the outer mints, arriving through the declared type rather than being made
//! again. [`build`] is the one body both the outer ascription and the nested case go through.

use crate::function::{KValue, Knotted, module};
use crate::memory::{BumpAllocator, BumpVec, ScopeId, Writer};
use crate::symbols::{BinderSymbol, TypeSymbol, ValueSymbol};
use crate::type_lattice::{
    KType, Members, SchemaDraft, SigSchema, SigSubtypeFailure, TypeNode, TypeRegistry,
    constructor_param_names, member as bound_member, sig_subtype, substitute_sig_members,
};
use crate::values::{TypeValue, Value};

use super::coerce::{Coercion, CoercionRefused, coerce};
use super::layout;

/// Which operator is ascribing: `:!` keeps the source's types, `:|` mints its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ascription {
    /// `:|` — each abstract member becomes a fresh mint, and every member is born coerced to it.
    Opaque,
    /// `:!` — the abstract members keep the source's bindings, so every member is carried verbatim.
    Transparent,
}

/// Why a module could not take a signature.
#[derive(Clone, Copy, Debug)]
pub enum Unascribable<'run, 'x> {
    /// The operand is no module.
    NotAModule,
    /// The ascribed handle names no signature.
    NotASignature(KType),
    /// The module does not satisfy the signature.
    Unsatisfied(SigSubtypeFailure<'run, 'x>),
    /// A member the signature names could not take the view's type for it.
    Coercion {
        name: ValueSymbol,
        refused: CoercionRefused,
    },
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
    let sig = layout::schema_of(signature, types).ok_or(Unascribable::NotASignature(signature))?;
    let held = source_schema(source, types).ok_or(Unascribable::NotAModule)?;
    sig_subtype(types, scratch, held, sig).map_err(Unascribable::Unsatisfied)?;
    let from = source_bindings(&sig, &held, scratch);
    let to = match mode {
        // Transparent: the abstract members keep the source's bindings, so every slot type reads
        // the same either side and the coercion walk stops at its first comparison.
        Ascription::Transparent => from,
        Ascription::Opaque => mint(&sig, types, scratch),
    };
    let view = view_signature(&sig, to, types, scratch);
    build(writer, source, sig, view, from, to, types, scratch)
}

/// Build the view's member run in layout order and lay the node down. `from` and `to` are what the
/// source and the view bind the signature's abstract members to.
///
/// Two callers: an ascription, and a nested signature slot inside one
/// ([`coerce`](super::coerce::coerce)), which passes the enclosing substitutions unchanged —
/// a nested boundary mints nothing of its own. Satisfaction is the caller's: an ascription checks
/// it outright, and a nested slot was checked when the enclosing one was.
#[allow(clippy::too_many_arguments)]
pub(super) fn build<'graph, 'cell, 'run, 'x>(
    writer: Writer<'cell>,
    source: Knotted<'graph, 'cell>,
    sig: SigSchema<'run>,
    view: KType,
    from: Members<'x, TypeSymbol>,
    to: Members<'x, TypeSymbol>,
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
    let view_schema = layout::schema_of(view, types).expect("the view's own signature");
    let mut members: BumpVec<'x, KValue<'graph, 'cell>> =
        BumpVec::with_capacity_in(layout::member_count(&view_schema, scratch), scratch);
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
    for (_, handle) in layout::type_members(&view_schema, scratch).iter().copied() {
        members.push(Value::Type(TypeValue::new(writer, handle, types)));
    }
    Ok(Knotted::of(module(writer, view, &members), 0))
}

/// A fresh mint per abstract member of `sig`: a rigid variable carrying this application's nonce,
/// over the same parameter names and the same bound the declaration gives it.
fn mint<'x>(
    sig: &SigSchema<'_>,
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'x>,
) -> Members<'x, TypeSymbol> {
    let nonce = ScopeId::next();
    Members::from_pairs(
        scratch,
        sig.abstract_members.iter().map(|(name, declared)| {
            let params = constructor_param_names(*declared, types).unwrap_or(&[]);
            let bound = match types.node(*declared) {
                TypeNode::AbstractType { bound, .. } => bound,
                _ => KType::ANY,
            };
            (
                *name,
                types.abstract_type(scratch, nonce, *name, params, Some(nonce), bound),
            )
        }),
    )
}

/// What `source` binds each of `sig`'s abstract members to — the substitution the member it holds
/// was built under.
fn source_bindings<'x>(
    sig: &SigSchema<'_>,
    source: &SigSchema<'_>,
    scratch: BumpAllocator<'x>,
) -> Members<'x, TypeSymbol> {
    Members::from_pairs(
        scratch,
        sig.abstract_members.iter().map(|(name, _)| {
            (
                *name,
                source
                    .type_member(*name)
                    .expect("satisfaction admitted every abstract member"),
            )
        }),
    )
}

/// The signature the view itself carries: every one of `sig`'s type members fixed manifest at what
/// `to` gives it, and every value slot at its declared type read under `to`. A module's own
/// signature never has abstract members, and a view's is a module's.
fn view_signature(
    sig: &SigSchema<'_>,
    to: Members<'_, TypeSymbol>,
    types: &TypeRegistry<'_>,
    scratch: BumpAllocator<'_>,
) -> KType {
    let read =
        |declared: KType| substitute_sig_members(types, scratch, declared, ScopeId::SENTINEL, to);
    let mut draft = SchemaDraft::new(scratch);
    for (name, _) in sig.abstract_members.iter().copied() {
        draft.insert_manifest(
            name,
            bound_member(to, name).expect("every abstract member is bound"),
        );
    }
    for (name, fixed) in sig.manifest_members.iter().copied() {
        draft.insert_manifest(name, read(fixed));
    }
    for (name, declared) in sig.value_slots.iter().copied() {
        draft.insert_value_slot(name, read(declared));
    }
    types.signature(scratch, draft)
}

/// The self-signature `source` carries, if it is a module.
fn source_schema<'run>(
    source: Knotted<'_, '_>,
    types: &TypeRegistry<'run>,
) -> Option<SigSchema<'run>> {
    layout::schema_of(source.module()?.ktype(), types)
}
