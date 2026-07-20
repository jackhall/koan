//! Ascription operators `:|` (opaque) and `:!` (transparent).
//! See [design/typing/modules.md](../../design/typing/modules.md).
//!
//! Satisfaction is checked through the signature-subtyping relation: the source module's
//! self-sig must be a subtype of the signature's schema (manifest members equal, abstract
//! members at the right kind and over the same parameter names, value slots covariantly
//! compatible). Each view also seals its own self-sig at creation.

use crate::machine::model::KType;
use crate::machine::model::TypeRegistry;
use crate::machine::model::{
    sig_subtype, substitute_sig_members, KKind, NominalMember, NominalSchema, RecursiveSet,
    SigContent, SigSchema,
};
use crate::machine::model::{KObject, Module};
use crate::machine::StepCarried;
use crate::machine::{KError, KErrorKind, Scope};
use std::collections::HashMap;
use std::rc::Rc;

use super::{arg, kw, sig};

/// `<m:Module> :| <s:Signature>` — opaque ascription. Reads `m` / `s` from the
/// `BodyCtx::args` type channel, mints on `ctx.scope.region`, and returns the view module as a
/// witnessed [`Action::Done(Ok)`](Action::Done) carrier ([`Scope::resident_type_carrier`] seals it under the
/// child scope's token, derived via [`Scope::child_module_reach`]).
pub fn body_opaque<'a>(ctx: &crate::machine::BodyCtx<'a, '_>) -> crate::machine::Action<'a> {
    use crate::machine::Action;

    let (m, s) = crate::try_action!(resolve_module_and_signature(ctx.args));

    let region = ctx.scope.brand();
    let new_scope = region.alloc_scope(Scope::child_under_module(
        ctx.scope,
        format!("{} :| {}", m.path, s.path),
    ));

    let src = m.child_scope();
    if let Err(e) = new_scope.bindings().try_bulk_install_from(src.bindings()) {
        return Action::Done(Err(e));
    }

    // The view's members are all bulk-installed into `new_scope` above, and nothing binds into it
    // below (the type-member / slot-tag writes target `new_module`, not the scope) — so seal its
    // reach-set here, before the module captures it, mirroring the MODULE / SIG block-finish close.
    // A member folded into the set rides the escaping view-module value sealed in.
    new_scope.close();

    let new_module: &'a Module<'a> = region.alloc_module(Module::new(m.path.clone(), new_scope));
    // Per-slot kind: a SIG-declared higher-kinded slot (`TYPE (Elem AS Wrap)`) mints a fresh
    // `TypeConstructor` family over the slot's declared parameter names rather than the default
    // `AbstractType` arm, preserving the higher-kinded shape across the ascription barrier.
    let mut minted: Vec<(String, KType)> = Vec::new();
    for (name, kt) in &s.schema.abstract_members {
        let minted_kt = match kt {
            KType::AbstractType { param_names, .. } if !param_names.is_empty() => {
                let member = NominalMember::pending(name.clone(), KKind::TypeConstructor);
                // Generative: the per-application nonce (the minted module's `scope_id`)
                // folds into the set digest, so two `:|` applications never unify.
                let fresh = RecursiveSet::new_generative(vec![member], new_module.scope_id());
                fresh.fill_member(
                    0,
                    NominalSchema::TypeConstructor {
                        schema: HashMap::new(),
                        param_names: param_names.clone(),
                    },
                );
                KType::SetRef {
                    set: std::rc::Rc::new(fresh),
                    index: 0,
                }
            }
            _ => KType::AbstractType {
                source: new_module.scope_id(),
                name: name.clone(),
                param_names: Vec::new(),
            },
        };
        minted.push((name.clone(), minted_kt));
    }
    // A manifest member reads concretely through the opaque view: the view scope carries no
    // type entries (`try_bulk_install_from` copies only the data table), so its fixed `KType`
    // is mirrored into `type_members` alongside the per-call abstract mints.
    let manifest: Vec<(String, KType)> = s
        .schema
        .manifest_members
        .iter()
        .map(|(n, t)| (n.clone(), t.clone()))
        .collect();
    if !minted.is_empty() || !manifest.is_empty() {
        let mut tm = new_module.type_members.borrow_mut();
        for (n, t) in minted {
            tm.insert(n, t);
        }
        for (n, t) in manifest {
            tm.insert(n, t);
        }
    }

    {
        let tm = new_module.type_members.borrow();
        let mut tags: Vec<(String, KType)> = Vec::new();
        for (slot_name, kt) in &s.schema.value_slots {
            if let KType::AbstractType { name: member, .. } = kt {
                if let Some(per_call) = tm.get(member) {
                    tags.push((slot_name.clone(), per_call.clone()));
                }
            }
        }
        drop(tm);
        if !tags.is_empty() {
            let mut stt = new_module.slot_type_tags.borrow_mut();
            for (slot_name, tag) in tags {
                stt.insert(slot_name, tag);
            }
        }
    }

    // Seal the view's self-sig after the type-member / slot-tag writes that feed the derivation.
    seal_view_self_sig(new_module, &s);

    if let Err(e) = check_satisfies(m, &s, ctx.types) {
        return Action::Done(Err(e));
    }

    // The view's token is derived from `new_scope` held directly here (co-located, so it names only
    // what the bulk-installed members reach and derives its home-borrow bit from the mint), stored
    // nowhere — the view is a returned value, not a named binding — and sealed onto the terminal
    // carrier, witnessing the module in place. The opaque view's `new_scope` is a same-region child
    // of this frame, so the derived bit records the module's home borrow.
    let stored = ctx.scope.child_module_reach(new_scope);
    // The view surfaces as the Object-arm module value (`new_module` lives in `region`'s own
    // region, so the audit passes on the dest-only check alone); a LET around it binds that value
    // like any other.
    let obj = crate::try_action!(ctx
        .scope
        .alloc_object_reaching(KObject::Module(new_module), &stored));
    Action::Done(Ok(StepCarried::born(
        ctx.scope.resident_value_carrier(obj, stored),
    )))
}

/// `<m:Module> :! <s:Signature>` — transparent ascription. Shape-checks against the source's
/// own child scope and returns the retagged view as the Object-arm module value — the terminal is
/// allocated reaching the token derived via [`Scope::child_module_reach`], which pins the (foreign)
/// source module's child-scope region the view borrows.
pub fn body_transparent<'a>(ctx: &crate::machine::BodyCtx<'a, '_>) -> crate::machine::Action<'a> {
    use crate::machine::Action;

    let (m, s) = crate::try_action!(resolve_module_and_signature(ctx.args));
    if let Err(e) = check_satisfies(m, &s, ctx.types) {
        return Action::Done(Err(e));
    }
    // A transparent view reuses the source module's child scope directly (`m.child_scope()`), foreign
    // to this frame — so its token folds that source's region and reach and derives its home-borrow
    // bit from the mint, sealed onto the terminal. Minted *before* the module alloc below: both the
    // module's own placement (its child scope is this foreign region, not `region`'s own) and the
    // Object-arm carrier that surfaces it need this one token. Reusing the foreign source's child scope, the view
    // borrows nothing into this home frame (its interior points at the source region), so the derived
    // bit stays unset — a downstream copied-mode mint materializes no home-frame member, and the dying
    // home frame frees once its retention hold releases.
    let stored = ctx.scope.child_module_reach(m.child_scope());
    let new_module: &'a Module<'a> = ctx.scope.alloc_module_reaching(
        Module::new(format!("{} :! {}", m.path, s.path), m.child_scope()),
        &stored,
    );
    // Seal the view's self-sig off the source child scope it reuses; SIG-declared value slots
    // read the source's concrete types after substitution.
    seal_view_self_sig(new_module, &s);
    // The view surfaces as the Object-arm module value under the same token that pins the reused
    // source's (foreign) child-scope region; a LET around it binds that value like any other.
    let obj = crate::try_action!(ctx
        .scope
        .alloc_object_reaching(KObject::Module(new_module), &stored));
    Action::Done(Ok(StepCarried::born(
        ctx.scope.resident_value_carrier(obj, stored),
    )))
}

/// Seal an ascription view's self-sig. The raw derivation captures the view's members; each
/// SIG-declared value slot is then re-expressed in the view's own type members — the SIG's
/// abstract-member references substituted by the view's bindings for them (an opaque view's
/// per-call abstract mints, a transparent view's concrete source types). Without this a slot
/// typed against an abstract member would read concrete off the underlying value and the view
/// would not structurally satisfy its own signature.
fn seal_view_self_sig<'a>(module: &Module<'a>, content: &SigContent) {
    let mut view_sig = SigSchema::raw_self_sig(module);
    let member_map: std::collections::HashMap<String, KType> = view_sig
        .manifest_members
        .iter()
        .map(|(n, t)| (n.clone(), t.clone()))
        .collect();
    for (slot_name, declared) in &content.schema.value_slots {
        view_sig.value_slots.insert(
            slot_name.clone(),
            substitute_sig_members(declared, content.sig_id, &member_map),
        );
    }
    module.seal_self_sig(view_sig);
}

/// Read the `m:Module` / `s:Signature` operands from the `BodyCtx::args` record: the module off the
/// value channel's Object arm, the signature off the type channel, producing a missing / mismatch
/// diagnostic when an operand is absent or the wrong kind.
fn resolve_module_and_signature<'a>(
    args: &crate::machine::model::KObject<'a>,
) -> Result<(&'a crate::machine::model::Module<'a>, Rc<SigContent>), KError> {
    use crate::machine::{arg_held, arg_object, arg_type};

    fn type_mismatch_or_missing(
        args: &crate::machine::model::KObject<'_>,
        name: &str,
        expected: &str,
    ) -> KError {
        match arg_held(args, name) {
            Some(held) => KError::new(KErrorKind::TypeMismatch {
                arg: name.to_string(),
                expected: expected.to_string(),
                got: held.ktype().name(),
            }),
            None => KError::new(KErrorKind::MissingArg(name.to_string())),
        }
    }

    let m = match arg_object(args, "m") {
        Some(KObject::Module(module)) => *module,
        _ => return Err(type_mismatch_or_missing(args, "m", "Module")),
    };
    let s = match arg_type(args, "s") {
        Some(KType::Signature { content, .. }) => Rc::clone(content),
        _ => return Err(type_mismatch_or_missing(args, "s", "Signature")),
    };
    Ok((m, s))
}

/// Verify a module satisfies `c` through the signature-subtyping relation: the module's
/// self-sig must be a subtype of the signature's bare schema (every member present, manifest
/// members equal, abstract members at the right kind and parameter names, value slots covariantly compatible
/// after abstract-member substitution). The decision (and its memoization) lives in
/// [`Module::satisfies_sig_content`], the shared entry point dispatch also routes through; this
/// function only rebuilds the `ShapeError` diagnostic on the cold path when that check fails.
fn check_satisfies<'a>(m: &Module<'a>, c: &SigContent, types: &TypeRegistry) -> Result<(), KError> {
    if m.satisfies_sig_content(c, types) {
        return Ok(());
    }
    match sig_subtype(m.self_sig(), &c.schema, types) {
        Ok(()) => unreachable!("a recorded false verdict must re-fail on the diagnostic walk"),
        Err(failure) => Err(KError::new(KErrorKind::ShapeError(format!(
            "module does not satisfy signature `{}`: {}",
            c.path,
            failure.render_fragment()
        )))),
    }
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    // Slots are typed `Module` / `Signature`. A bare module operand (`int_ord :| Ordered`) is an
    // Identifier that resolves value-side and rides the auto-wrap rails into a value-typed future,
    // so no parallel Type-Type overload is required.
    let opaque_sig = sig(
        KType::empty_signature(),
        vec![
            arg("m", KType::empty_signature()),
            kw(":|"),
            arg("s", KType::OfKind(KKind::Signature)),
        ],
    );
    let transparent_sig = sig(
        KType::empty_signature(),
        vec![
            arg("m", KType::empty_signature()),
            kw(":!"),
            arg("s", KType::OfKind(KKind::Signature)),
        ],
    );
    crate::builtins::register_builtin(scope, ":|", opaque_sig, body_opaque);
    crate::builtins::register_builtin(scope, ":!", transparent_sig, body_transparent);
}

#[cfg(test)]
mod tests;
