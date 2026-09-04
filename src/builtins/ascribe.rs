//! Ascription operators `:|` (opaque) and `:!` (transparent).
//! See [design/typing/modules.md](../../design/typing/modules.md).
//!
//! Satisfaction is checked through the signature-subtyping relation: the source module's
//! self-sig must be a subtype of the signature's schema (manifest members equal, abstract
//! members at the right kind and over the same parameter names, value slots covariantly
//! compatible). Each view is born carrying its own self-sig, derived from the members its
//! construction gathered before the view module exists.
//!
//! Both operators build the same thing: a scope of the view's own, holding exactly the members the
//! signature declares. They differ only in what the signature's abstract members are seeded at —
//! per-call mints for `:|`, the source's own bindings for `:!` — and everything else, pruning
//! included, follows from that one choice. Width subtyping is what admits a module that binds more
//! than it declares; it is not a property of the view the match produces.

use std::borrow::Cow;

use crate::machine::StepCarried;
use crate::machine::WriteGate;
use crate::machine::core::ViewMembers;
use crate::machine::model::KType;
use crate::machine::model::TypeRegistry;
use crate::machine::model::{
    KKind, MemberCoercion, RecursiveGroupWindow, RelativeSchema, SigSchema, TypeNode,
    canonical_overloads, sig_subtype, substitute_sig_members,
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
    ascribe(ctx, Transparency::Opaque)
}

/// `<m:Module> :! <s:Signature>` — transparent ascription. The same view construction the opaque
/// operator performs, seeded with the *source's* own bindings for the signature's abstract members
/// rather than per-call mints: the barrier's two sides coincide, so nothing coerces and every
/// declared member replays verbatim at the concrete types the source binds. Width is still pruned —
/// the view surfaces exactly what the signature declares.
pub fn body_transparent<'a>(
    ctx: &crate::machine::BodyCtx<'_, 'a, '_>,
) -> crate::machine::Action<'a> {
    ascribe(ctx, Transparency::Transparent)
}

/// Which types a view's abstract members are seeded at — the one axis the two ascription operators
/// differ on, everything downstream of the seeding being shared.
#[derive(Clone, Copy)]
enum Transparency {
    /// A fresh per-call mint per abstract member: the source's representation is hidden.
    Opaque,
    /// The source's own binding per abstract member: `view.Carrier` reads through to `Number`.
    Transparent,
}

/// Build an ascription view of `m` at signature `s`: check satisfaction, allocate the view scope
/// and replay into it the members the signature declares — at the types `mode` seeds its abstract
/// members with — then derive the view's self-sig and return the view module as the Object-arm
/// value.
fn ascribe<'a>(
    ctx: &crate::machine::BodyCtx<'_, 'a, '_>,
    mode: Transparency,
) -> crate::machine::Action<'a> {
    use crate::machine::Action;

    let (m, s) = crate::try_action!(resolve_module_and_signature(ctx.args, ctx.registries));
    with_signature_schema(s, ctx.types(), |s_schema, s_digest| {
        // Checked before the view is built, so the coercion plan below is founded on a module that
        // genuinely supplies every member the signature names.
        if let Err(e) = check_satisfies(m, s_schema, s_digest, s, ctx.registries) {
            return Action::done(Err(e));
        }
        // The source side of the barrier: what the *module* binds each abstract member of the
        // ascribed signature to. Read off the module's own self-sig, which is the same table
        // satisfaction substituted through above.
        let source_members = source_member_bindings(m, s_schema, ctx.types());
        let sig_id = s_schema.sig_id.unwrap_or(ScopeId::SENTINEL);

        // Allocate the view scope, replay into it the members the signature declares — each one
        // whose slot type substitutes differently under the view's own bindings born *coerced* —
        // and seed its `types` table with the view's own type members, all in one door: the scope
        // is unreachable until the whole install has landed, which is what lets the writes happen
        // at construction time rather than riding a step outcome. The seeded table is what a
        // `USING` window over this view borrows, so the view's members resolve by bare name in the
        // block, read at the view's types, and the source's representation types are absent rather
        // than masked.
        let new_scope = match Scope::alloc_module_view(
            ctx.scope,
            m.child_scope(),
            ctx.registries,
            // The nonce every per-call mint carries is the newborn view scope's id, handed in by
            // the door before the scope is published (`Module::scope_id` reports the same id once
            // the module is built); the transparent seeding mints nothing and ignores it. Abstract
            // and manifest members are disjoint by `SigSchema` construction — a SIG member is one
            // or the other — so the strict inserts cannot collide.
            |nonce| {
                let types = match mode {
                    Transparency::Opaque => view_type_members(s_schema, nonce, ctx.registries),
                    Transparency::Transparent => source_type_members(s_schema, &source_members),
                };
                view_members(s_schema, sig_id, &source_members, types, ctx.registries)
            },
        ) {
            Ok(scope) => scope,
            Err(e) => return Action::done(Err(e)),
        };

        // The view's members are all installed into `new_scope` above, and nothing binds into it
        // below (the slot-tag writes target `new_module`, not the scope) — so seal its reach-set
        // here, before the module captures it, mirroring the MODULE / SIG block-finish close. A
        // member folded into the set rides the escaping view-module value sealed in.
        new_scope.close();

        // `type_members` mirrors the child scope's type bindings, as it does for a plain module
        // built by `module_def.rs`: the seeded entries are read straight back out.
        let mut draft = ModuleDraft::empty();
        for (name, kt) in new_scope.bindings().iter_types() {
            draft.type_members.insert(name, kt);
        }

        // The view's self-sig is derived from the draft the seeding just filled, then the module
        // is born carrying it.
        let self_sig = view_self_sig(new_scope, &draft, s_schema, ctx.registries);
        // A transparent view is spelled into its own path: the source's representation reads
        // through it, so the label is what tells the two apart where a module is rendered.
        let path: Cow<'_, str> = match mode {
            Transparency::Opaque => Cow::Borrowed(m.path),
            Transparency::Transparent => {
                Cow::Owned(format!("{} :! {}", m.path, s.name(ctx.registries)))
            }
        };
        let new_module: &'a Module<'a> =
            Module::alloc_at_child_scope(&path, new_scope, draft, self_sig);

        // The view surfaces as the Object-arm module value; a LET around it binds that value like
        // any other. The store door merges the resident module reference — enveloped at
        // `new_scope`'s own home, held directly here rather than recovered by walking the built
        // value — into this scope's region, so the composition mints and retains the module's
        // reach. A view's `new_scope` is a same-region child of this frame, so that reach
        // is this scope's own region: the module genuinely borrows into its home. Lifting the seal
        // upgrades the description's members `Weak → Rc` for the terminal the step delivers onward.
        let sealed = ctx.scope.store_module_object(new_module);
        Action::done(Ok(StepCarried::born_delivered(
            ctx.scope.lift_resident(sealed),
        )))
    })
}

/// The type members a transparent view's scope is seeded with: every abstract member of the
/// ascribed signature at the source's own binding for it, plus every manifest member at its fixed
/// `KType`. Seeding the source's bindings is what makes the view transparent — `view.Carrier` and
/// the source's `Carrier` are one type — and it collapses the coercion plan to the identity, so
/// every declared member replays verbatim.
///
/// `source_members` is total over the signature's abstract members: satisfaction admitted this
/// module, and a module's own type bindings are all manifest in its self-sig.
fn source_type_members(
    signature: &SigSchema,
    source_members: &TypeMemberMap,
) -> Vec<(TypeSymbol, KType)> {
    signature
        .abstract_members
        .keys()
        .filter_map(|name| source_members.get(name).map(|kt| (*name, *kt)))
        .chain(signature.manifest_members.iter().map(|(n, kt)| (*n, *kt)))
        .collect()
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
        // The slot's node is read in place and the mint runs under the read — interning inside one
        // is legal — so nothing copies but the parameter names, which the minted family owns.
        let minted_kt = types.with_node(*kt, |node| match node {
            TypeNode::AbstractType { param_names, .. } if !param_names.is_empty() => {
                // The mint carries the SIG member's own classified name straight across: the
                // member it re-declares is the same Type-class label the SIG declaration interned.
                RecursiveGroupWindow::seal_singleton(
                    *name,
                    RelativeSchema::TypeConstructor {
                        schema: TypeMemberMap::default(),
                        param_names: param_names.clone(),
                    },
                    Some(nonce),
                    types,
                )
            }
            // Generative by the same mechanism as the higher-kinded arm above. `source` stays the
            // declaring SIG's binder — the two meanings ride separate fields.
            TypeNode::AbstractType { source, .. } => types.intern(TypeNode::AbstractType {
                source: *source,
                name: *name,
                param_names: Vec::new(),
                nonce: Some(nonce),
            }),
            // Unreachable: `is_abstract_sig_member` admits only `AbstractType` into
            // `abstract_members`, so the two arms above are exhaustive over this map.
            _ => *kt,
        });
        members.push((*name, minted_kt));
    }
    // A manifest member reads concretely through the opaque view: its declared identity is the
    // view's, unhidden.
    for (name, kt) in &signature.manifest_members {
        members.push((*name, *kt));
    }
    members
}

/// What the source module binds each abstract member of the ascribed signature to — the `from`
/// side of every coercion the view's replay performs. Read off the module's own self-sig, which is
/// the table [`check_satisfies`] substituted the signature's slot types through, so the two sides
/// of the barrier are decided from one place.
fn source_member_bindings(
    m: &Module<'_>,
    signature: &SigSchema,
    types: &TypeRegistry,
) -> TypeMemberMap {
    m.with_self_sig(types, |mine| {
        signature
            .abstract_members
            .keys()
            .filter_map(|name| mine.manifest_members.get(name).map(|kt| (*name, *kt)))
            .collect()
    })
}

/// The view's members, decided once its scope id is known: the seeded type members and the two
/// member bindings a coerced read is rewritten between, handed to
/// [`ViewMembers::of_schema`](crate::machine::core::ViewMembers) to derive what the view surfaces
/// and which of it is born coerced.
///
/// A member is coerced exactly when its declared type substitutes differently under the two
/// bindings — so a concrete slot (`VAL size :Number`), a manifest-typed slot, and every slot or
/// head naming no abstract member installs the source's own seal. `view_types` is
/// [`view_type_members`]'s output, consumed here and handed on as the plan's own field.
fn view_members(
    signature: &SigSchema,
    sig_id: ScopeId,
    source_members: &TypeMemberMap,
    view_types: Vec<(TypeSymbol, KType)>,
    registries: &RunRegistries,
) -> ViewMembers {
    // Only the abstract members are substitution points: a manifest member is a concrete type a
    // slot names directly, never a reference the walk rewrites.
    let view_members: TypeMemberMap = view_types
        .iter()
        .filter(|(name, _)| signature.abstract_members.contains_key(name))
        .copied()
        .collect();
    let coercion = MemberCoercion::new(sig_id, source_members, &view_members, &registries.types);
    ViewMembers::of_schema(signature, view_types, coercion, registries)
}

/// Intern an ascription view's self-sig, from the child scope it is being built over and the
/// members its construction gathered. The raw derivation captures the view's members; each
/// SIG-declared value slot and keyworded member is then re-expressed in the view's own type
/// members — the SIG's abstract-member references substituted by the view's bindings for them (an
/// opaque view's per-call abstract mints, a transparent view's concrete source types). Without this
/// a member typed against an abstract member would read concrete off the underlying value and the
/// view would not structurally satisfy its own signature.
///
/// The re-expression is what the view *publishes*, and it is deliberately the declared shape rather
/// than the installed one: a bucket entry replayed verbatim may be more general than the member it
/// was selected for, and reporting that generality would widen the view's interface past what the
/// ascription promised.
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
    for (key, declared_overloads) in &signature.keyworded {
        let substituted = declared_overloads
            .iter()
            .map(|declared| substitute_sig_members(*declared, sig_id, &member_map, types))
            .collect();
        view_sig
            .keyworded
            .insert(key.clone(), canonical_overloads(substituted));
    }
    types.signature(view_sig)
}

/// Read the schema and its content digest off the signature handle `s` **in place**, and hand back
/// whatever `read` derives. `s` rode the `s` slot typed `OfKind(Signature)`, so its node is always
/// a `Signature`.
///
/// Both ascription bodies run inside this read: nothing they derive from the schema needs to own
/// it, and the mints and interning they perform are legal under a read.
fn with_signature_schema<R>(
    s: KType,
    types: &TypeRegistry,
    read: impl FnOnce(&SigSchema, crate::machine::model::TypeDigest) -> R,
) -> R {
    types.with_node(s, |node| match node {
        TypeNode::Signature {
            schema,
            schema_digest,
            ..
        } => read(schema, *schema_digest),
        _ => unreachable!("the `s` operand is `OfKind(Signature)`; only a signature handle admits"),
    })
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
        Some(kt) if types.with_node(kt, |node| matches!(node, TypeNode::Signature { .. })) => kt,
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
    signature: KType,
    registries: &RunRegistries,
) -> Result<(), KError> {
    let types = &registries.types;
    if m.satisfies_sig_schema(schema, schema_digest, registries) {
        return Ok(());
    }
    match sig_subtype(&m.self_sig(types), schema, registries) {
        Ok(()) => unreachable!("a recorded false verdict must re-fail on the diagnostic walk"),
        // The signature's spelling is written into the message here rather than at the call: an
        // ascription that holds names it nowhere, and this arm is the only reader.
        Err(failure) => Err(KError::new(KErrorKind::ShapeError(format!(
            "module does not satisfy signature `{}`: {}",
            signature.display_name(registries),
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
