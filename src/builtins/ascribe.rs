//! Ascription operators `:|` (opaque) and `:!` (transparent).
//! See [design/typing/modules.md](../../design/typing/modules.md).
//!
//! Satisfaction is checked through the signature-subtyping relation: the source module's
//! self-sig must be a subtype of the signature's schema (manifest members equal, abstract
//! members at the right kind and over the same parameter names, value slots covariantly
//! compatible). Each view is born carrying its own self-sig, derived from the members its
//! construction gathered before the view module exists.

use crate::machine::StepCarried;
use crate::machine::WriteGate;
use crate::machine::model::KType;
use crate::machine::model::TypeRegistry;
use crate::machine::model::{
    KKind, RecursiveGroupWindow, RelativeSchema, SigSchema, TypeNode, sig_subtype,
    substitute_sig_members,
};
use crate::machine::model::{KObject, Module, ModuleDraft};
use crate::machine::model::{TypeMemberMap, TypeSymbol, ValueSymbol};
use crate::machine::{KError, KErrorKind, Scope, ScopeId};

use super::{arg, kw, sig};
use crate::machine::BoundArgs;
use crate::machine::model::RunRegistries;
use crate::machine::model::StaticName;

// This builtin's slot spellings, minted once and read back by symbol.
crate::slots! { SLOTS { m, s } }

/// `<m:Module> :| <s:Signature>` — opaque ascription. Reads `m` / `s` from the
/// `BodyCtx::args` type channel, mints on `ctx.scope.region`, and returns the view module as a
/// witnessed [`Action::done(Ok)`](Action::Done) carrier ([`Scope::store_module_object`] merges the
/// resident module reference into that region, composing its reach).
pub fn body_opaque<'a>(ctx: &crate::machine::BodyCtx<'_, 'a, '_>) -> crate::machine::Action<'a> {
    use crate::machine::Action;

    let (m, s) = crate::try_action!(resolve_module_and_signature(ctx.args, ctx.registries));
    let (s_schema, s_digest) = signature_schema(s, ctx.types());
    let s_name = s.name(ctx.registries);

    // Allocate the view scope, replay the source module's members into it, and seed its `types`
    // table with the view's own type members, all in one door: the scope is unreachable until the
    // whole install has landed, which is what lets the writes happen at construction time rather
    // than riding a step outcome. The seeded table is what a `USING` window over this view borrows,
    // so the view's members resolve by bare name in the block and the source's representation types
    // are absent rather than masked.
    let new_scope = match Scope::alloc_module_view(
        ctx.scope,
        m.child_scope().bindings(),
        ctx.registries,
        // The nonce every per-call mint carries is the newborn view scope's id, handed in by the
        // door before the scope is published (`Module::scope_id` reports the same id once the
        // module is built). Abstract and manifest members are disjoint by `SigSchema` construction
        // — a SIG member is one or the other — so the strict inserts cannot collide.
        |nonce| view_type_members(&s_schema, nonce, ctx.registries),
    ) {
        Ok(scope) => scope,
        Err(e) => return Action::done(Err(e)),
    };

    // The view's members are all installed into `new_scope` above, and nothing binds into it below
    // (the slot-tag writes target `new_module`, not the scope) — so seal its reach-set here, before
    // the module captures it, mirroring the MODULE / SIG block-finish close. A member folded into
    // the set rides the escaping view-module value sealed in.
    new_scope.close();

    // `type_members` mirrors the child scope's type bindings, as it does for a plain module built
    // by `module_def.rs`: the seeded entries are read straight back out.
    let mut draft = ModuleDraft::empty();
    for (name, kt) in new_scope.bindings().iter_types() {
        draft.type_members.insert(name, kt);
    }

    // A slot's tag is keyed by the slot's own name, its value by the per-call mint the abstract
    // member resolved to — the schema and the draft share the classified currency, so both keys
    // travel straight across.
    let mut tags: Vec<(ValueSymbol, KType)> = Vec::new();
    for (slot_name, kt) in &s_schema.value_slots {
        if let TypeNode::AbstractType { name: member, .. } = ctx.types().node(*kt)
            && let Some(per_call) = draft.type_members.get(&member)
        {
            tags.push((*slot_name, *per_call));
        }
    }
    for (slot_name, tag) in tags {
        draft.slot_type_tags.insert(slot_name, tag);
    }

    // The view's self-sig is derived from the draft the mints and slot tags just filled, then the
    // module is born carrying it.
    let self_sig = view_self_sig(new_scope, &draft, &s_schema, ctx.registries);
    let new_module: &'a Module<'a> =
        Module::alloc_at_child_scope(m.path, new_scope, draft, self_sig);

    if let Err(e) = check_satisfies(m, &s_schema, s_digest, &s_name, ctx.registries) {
        return Action::done(Err(e));
    }

    // The view surfaces as the Object-arm module value; a LET around it binds that value like any
    // other. The store door merges the resident module reference — enveloped at `new_scope`'s own
    // home, held directly here rather than recovered by walking the built value — into this scope's
    // region, so the composition mints and retains the module's reach. The opaque view's
    // `new_scope` is a same-region child of this frame, so that reach is this scope's own region:
    // the module genuinely borrows into its home. Lifting the seal upgrades the description's
    // members `Weak → Rc` for the terminal the step delivers onward.
    let sealed = ctx.scope.store_module_object(new_module);
    Action::done(Ok(StepCarried::born_delivered(
        ctx.scope.lift_resident(sealed),
    )))
}

/// `<m:Module> :! <s:Signature>` — transparent ascription. Shape-checks against the source's
/// own child scope and returns the retagged view as the Object-arm module value — built at the fold
/// brand of [`Scope::store_transparent_view`], whose composition pins the (foreign) source module's
/// child-scope region the view borrows.
pub fn body_transparent<'a>(
    ctx: &crate::machine::BodyCtx<'_, 'a, '_>,
) -> crate::machine::Action<'a> {
    use crate::machine::Action;

    let (m, s) = crate::try_action!(resolve_module_and_signature(ctx.args, ctx.registries));
    let (s_schema, s_digest) = signature_schema(s, ctx.types());
    let s_name = s.name(ctx.registries);
    if let Err(e) = check_satisfies(m, &s_schema, s_digest, &s_name, ctx.registries) {
        return Action::done(Err(e));
    }
    // A transparent view re-tags the source module's child scope directly (`m.child_scope()`),
    // foreign to this frame. The store door merges that scope as its source operand, so the
    // re-tagged Module and its Object-arm wrapper are both built at one fold brand in this scope's
    // region over that operand view — their borrow of the foreign region is the fold's own, and the
    // composition mints it as the value's reach and retains it here. The view's self-sig is sealed
    // inside the fold, on the resident module (SIG-declared value slots read the source's concrete
    // types after substitution). Reusing the foreign source's child scope, the view borrows nothing
    // into this home frame, so no home member is named and the dying home frame frees at its own
    // finalize, once the delivery walk has adopted the terminal onward. Lifting the seal upgrades
    // the description's members `Weak → Rc` for
    // the terminal the step delivers onward.
    let sealed = ctx.scope.store_transparent_view(
        format!("{} :! {}", m.path, s_name),
        m.child_scope(),
        |scope_view| view_self_sig(scope_view, &ModuleDraft::empty(), &s_schema, ctx.registries),
    );
    Action::done(Ok(StepCarried::born_delivered(
        ctx.scope.lift_resident(sealed),
    )))
}

/// The type members an opaque view's scope is seeded with: a per-call mint for every abstract
/// member of the ascribed signature, plus every manifest member at its fixed `KType`. `nonce` is
/// the view scope's id — folded into each mint's digest, so two `:|` applications of one signature
/// never unify.
fn view_type_members(
    signature: &SigSchema,
    nonce: ScopeId,
    registries: &RunRegistries,
) -> Vec<(TypeSymbol, KType)> {
    let types = &registries.types;
    let mut members: Vec<(TypeSymbol, KType)> = Vec::new();
    // Per-slot kind: a SIG-declared higher-kinded slot (`TYPE (Elem AS Wrap)`) mints a fresh
    // `TypeConstructor` family over the slot's declared parameter names rather than the default
    // `AbstractType` arm, preserving the higher-kinded shape across the ascription barrier.
    for (name, kt) in &signature.abstract_members {
        let minted_kt = match types.node(*kt) {
            TypeNode::AbstractType { param_names, .. } if !param_names.is_empty() => {
                // The mint carries the SIG member's own classified name straight across: the
                // member it re-declares is the same Type-class label the SIG declaration interned.
                RecursiveGroupWindow::seal_singleton(
                    *name,
                    RelativeSchema::TypeConstructor {
                        schema: TypeMemberMap::default(),
                        param_names,
                    },
                    Some(nonce),
                    types,
                )
            }
            // Generative by the same mechanism as the higher-kinded arm above. `source` stays the
            // declaring SIG's binder — the two meanings ride separate fields.
            TypeNode::AbstractType { source, .. } => types.intern(TypeNode::AbstractType {
                source,
                name: *name,
                param_names: Vec::new(),
                nonce: Some(nonce),
            }),
            // Unreachable: `is_abstract_sig_member` admits only `AbstractType` into
            // `abstract_members`, so the two arms above are exhaustive over this map.
            _ => *kt,
        };
        members.push((*name, minted_kt));
    }
    // A manifest member reads concretely through the opaque view: its declared identity is the
    // view's, unhidden.
    for (name, kt) in &signature.manifest_members {
        members.push((*name, *kt));
    }
    members
}

/// Intern an ascription view's self-sig, from the child scope it is being built over and the
/// members its construction gathered. The raw derivation captures the view's members; each
/// SIG-declared value slot is then re-expressed in the view's own type members — the SIG's
/// abstract-member references substituted by the view's bindings for them (an opaque view's
/// per-call abstract mints, a transparent view's concrete source types). Without this a slot
/// typed against an abstract member would read concrete off the underlying value and the view
/// would not structurally satisfy its own signature.
fn view_self_sig(
    child_scope: &Scope<'_>,
    draft: &ModuleDraft,
    signature: &SigSchema,
    registries: &RunRegistries,
) -> KType {
    let types = &registries.types;
    let mut view_sig = SigSchema::raw_self_sig(child_scope, draft);
    let member_map: crate::machine::model::TypeMemberMap = view_sig
        .manifest_members
        .iter()
        .map(|(n, t)| (*n, *t))
        .collect();
    // SIG-own abstract members canonicalize to `ScopeId::SENTINEL`; the empty interface names no
    // member, so its (empty) slot loop never substitutes.
    let sig_id = signature.sig_id.unwrap_or(ScopeId::SENTINEL);
    for (slot_name, declared) in &signature.value_slots {
        view_sig.value_slots.insert(
            *slot_name,
            substitute_sig_members(*declared, sig_id, &member_map, types),
        );
    }
    types.signature(view_sig)
}

/// The bare schema and its content digest carried by the signature handle `s`. `s` rode the `s`
/// slot typed `OfKind(Signature)`, so its node is always a `Signature`.
fn signature_schema(
    s: KType,
    types: &TypeRegistry,
) -> (SigSchema, crate::machine::model::TypeDigest) {
    match types.node(s) {
        TypeNode::Signature {
            schema,
            schema_digest,
            ..
        } => (schema, schema_digest),
        _ => unreachable!("the `s` operand is `OfKind(Signature)`; only a signature handle admits"),
    }
}

/// Read the `m:Module` / `s:Signature` operands from the `BodyCtx::args` record: the module off the
/// value channel's Object arm, the signature off the type channel, producing a missing / mismatch
/// diagnostic when an operand is absent or the wrong kind.
fn resolve_module_and_signature<'a>(
    args: BoundArgs<'a, '_>,
    registries: &crate::machine::model::RunRegistries,
) -> Result<(&'a crate::machine::model::Module<'a>, KType), KError> {
    let types = &registries.types;

    fn type_mismatch_or_missing(
        args: BoundArgs<'_, '_>,
        name: &StaticName<ValueSymbol>,
        expected: &str,
        registries: &crate::machine::model::RunRegistries,
    ) -> KError {
        match args.held(name) {
            Some(held) => KError::new(KErrorKind::TypeMismatch {
                arg: name.text().to_string(),
                expected: expected.to_string(),
                got: held.ktype(&registries.types).name(registries),
            }),
            None => KError::new(KErrorKind::MissingArg(name.text().to_string())),
        }
    }

    let m = match args.object(&SLOTS.m) {
        Some(KObject::Module(module)) => *module,
        _ => {
            return Err(type_mismatch_or_missing(
                args, &SLOTS.m, "Module", registries,
            ));
        }
    };
    let s = match args.ktype(&SLOTS.s) {
        Some(kt) if matches!(types.node(kt), TypeNode::Signature { .. }) => kt,
        _ => {
            return Err(type_mismatch_or_missing(
                args,
                &SLOTS.s,
                "Signature",
                registries,
            ));
        }
    };
    Ok((m, s))
}

/// Verify a module satisfies the interface `schema` (content digest `schema_digest`) through the
/// signature-subtyping relation: the module's self-sig must be a subtype of the bare schema (every
/// member present, manifest members equal, abstract members at the right kind and parameter names,
/// value slots covariantly compatible after abstract-member substitution). The decision (and its
/// memoization) lives in [`Module::satisfies_sig_schema`], the shared entry point dispatch also
/// routes through; this function only rebuilds the `ShapeError` diagnostic on the cold path when
/// that check fails. `sig_name` is the signature's rendered name for the diagnostic.
fn check_satisfies<'a>(
    m: &Module<'a>,
    schema: &SigSchema,
    schema_digest: crate::machine::model::TypeDigest,
    sig_name: &str,
    registries: &RunRegistries,
) -> Result<(), KError> {
    let types = &registries.types;
    if m.satisfies_sig_schema(schema, schema_digest, registries) {
        return Ok(());
    }
    match sig_subtype(&m.self_sig(types), schema, registries) {
        Ok(()) => unreachable!("a recorded false verdict must re-fail on the diagnostic walk"),
        Err(failure) => Err(KError::new(KErrorKind::ShapeError(format!(
            "module does not satisfy signature `{}`: {}",
            sig_name,
            failure.render_fragment()
        )))),
    }
}

pub fn register<'a>(scope: &'a Scope<'a>, registries: &RunRegistries, gate: &mut WriteGate) {
    // Slots are typed `Module` / `Signature`. A bare module operand (`int_ord :| Ordered`) is an
    // Identifier that resolves value-side and rides the auto-wrap rails into a value-typed future,
    // so no parallel Type-Type overload is required.
    let opaque_sig = sig(
        KType::EMPTY_SIGNATURE,
        vec![
            arg(registries, &SLOTS.m, KType::EMPTY_SIGNATURE),
            kw(registries, ":|"),
            arg(registries, &SLOTS.s, KType::of_kind(KKind::Signature)),
        ],
    );
    let transparent_sig = sig(
        KType::EMPTY_SIGNATURE,
        vec![
            arg(registries, &SLOTS.m, KType::EMPTY_SIGNATURE),
            kw(registries, ":!"),
            arg(registries, &SLOTS.s, KType::of_kind(KKind::Signature)),
        ],
    );
    crate::builtins::register_builtin(scope, opaque_sig, body_opaque, registries, gate);
    crate::builtins::register_builtin(scope, transparent_sig, body_transparent, registries, gate);
}

#[cfg(test)]
mod tests;
