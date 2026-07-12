//! Ascription operators `:|` (opaque) and `:!` (transparent).
//! See [design/typing/modules.md](../../design/typing/modules.md).
//!
//! Shape-checking is name-presence only; full type-shape checks are deferred to
//! the inference scheduler.

use crate::machine::execute::StepCarried;
use crate::machine::model::types::{
    AbstractSource, KKind, NominalMember, NominalSchema, ProjectedSchema, RecursiveSet,
};
use crate::machine::model::values::Module;
use crate::machine::model::KType;
use crate::machine::{KError, KErrorKind, NameLookup, Scope, ScopeId};

use super::{arg, kw, sig};

/// `<m:Module> :| <s:Signature>` — opaque ascription. Reads `m` / `s` from the
/// `BodyCtx::args` type channel, mints on `ctx.scope.region`, and returns the view module as a
/// witnessed [`Action::Done(Ok)`](Action::Done) carrier ([`Scope::resident_type_carrier`] seals it under the
/// child scope's token, derived via [`Scope::child_module_reach`]).
pub fn body_opaque<'a>(
    ctx: &crate::machine::core::kfunction::action::BodyCtx<'a, '_>,
) -> crate::machine::core::kfunction::action::Action<'a> {
    use crate::machine::core::kfunction::action::Action;

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
    // Per-slot kind: a SIG-declared higher-kinded slot (`TYPE (Type AS Wrap)`) mints a fresh
    // `TypeConstructor` rather than the default `AbstractType` arm, preserving the
    // higher-kinded shape across the ascription barrier.
    let mut minted: Vec<(String, KType<'a>)> = Vec::new();
    {
        let sig_bindings = s.decl_scope().bindings();
        for name in abstract_members_of(s.decl_scope()) {
            let kt = match sig_bindings
                .lookup_type(&name, None)
                .and_then(NameLookup::bound)
            {
                Some(KType::SetRef { set, index })
                    if set.member(*index).kind == KKind::TypeConstructor
                        && set.member(*index).scope_id == ScopeId::SENTINEL =>
                {
                    let ProjectedSchema::TypeConstructor {
                        schema,
                        param_names,
                    } = RecursiveSet::projected_schema(set, *index)
                    else {
                        unreachable!(
                            "TypeConstructor-kind member projects a TypeConstructor schema"
                        )
                    };
                    let member = NominalMember::pending(
                        name.clone(),
                        new_module.scope_id(),
                        KKind::TypeConstructor,
                    );
                    member.fill(NominalSchema::TypeConstructor {
                        schema,
                        param_names,
                    });
                    let fresh = std::rc::Rc::new(RecursiveSet::new(vec![member]));
                    KType::SetRef {
                        set: fresh,
                        index: 0,
                    }
                }
                _ => KType::AbstractType {
                    source: AbstractSource::Module(new_module),
                    name: name.clone(),
                },
            };
            minted.push((name.clone(), kt));
        }
    }
    // A manifest member reads concretely through the opaque view: the view scope carries no
    // type entries (`try_bulk_install_from` copies only the data table), so its fixed `KType`
    // is mirrored into `type_members` alongside the per-call abstract mints.
    let manifest = manifest_type_members_of(s.decl_scope());
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
        let mut tags: Vec<(String, KType<'a>)> = Vec::new();
        for (slot_name, kt) in s.decl_scope().bindings().iter_types() {
            if crate::parse::is_type_name(&slot_name) {
                continue;
            }
            if let KType::AbstractType {
                source: AbstractSource::Sig(_),
                name: member,
            } = kt
            {
                if let Some(per_call) = tm.get(member) {
                    tags.push((slot_name, per_call.clone()));
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

    if let Err(e) = shape_check(s, src) {
        return Action::Done(Err(e));
    }

    new_module.mark_satisfies(s.sig_id());

    // The view's token is derived from `new_scope` held directly here (co-located, so it names only
    // what the bulk-installed members reach and derives its home-borrow bit from the mint), stored
    // nowhere — the view is a returned value, not a named binding — and sealed onto the terminal
    // carrier, witnessing the module in place. The opaque view's `new_scope` is a same-region child
    // of this frame, so the derived bit records the module's home borrow.
    let stored = ctx.scope.child_module_reach(new_scope);
    // `new_module` lives in `region`'s own region (it was allocated into `new_scope`, itself
    // `region`-resident above), so the checked audit passes on the dest-only check alone.
    let kt_ref =
        crate::try_action!(region.alloc_ktype_checked(KType::Module { module: new_module }));
    Action::Done(Ok(StepCarried::born(
        ctx.scope.resident_type_carrier(kt_ref, stored),
    )))
}

/// `<m:Module> :! <s:Signature>` — transparent ascription. Shape-checks against the source's
/// own child scope and returns the retagged view module as a witnessed [`Action::Done(Ok)`](Action::Done)
/// carrier — [`Scope::resident_type_carrier`] pins the (foreign) source module's child-scope region
/// the view borrows, from the token derived via [`Scope::child_module_reach`].
pub fn body_transparent<'a>(
    ctx: &crate::machine::core::kfunction::action::BodyCtx<'a, '_>,
) -> crate::machine::core::kfunction::action::Action<'a> {
    use crate::machine::core::kfunction::action::Action;

    let (m, s) = crate::try_action!(resolve_module_and_signature(ctx.args));
    if let Err(e) = shape_check(s, m.child_scope()) {
        return Action::Done(Err(e));
    }
    // A transparent view reuses the source module's child scope directly (`m.child_scope()`), foreign
    // to this frame — so its token folds that source's region and reach and derives its home-borrow
    // bit from the mint, sealed onto the terminal. Minted *before* the module alloc below: both the
    // module's own placement (its child scope is this foreign region, not `region`'s own) and the
    // wrapping `KType::Module` need this one token. Reusing the foreign source's child scope, the view
    // borrows nothing into this home frame (its interior points at the source region), so the derived
    // bit stays unset — a downstream copied-mode mint materializes no home-frame member, and the dying
    // home frame frees once its retention hold releases.
    let stored = ctx.scope.child_module_reach(m.child_scope());
    let new_module: &'a Module<'a> = ctx.scope.alloc_module_reaching(
        Module::new(format!("{} :! {}", m.path, s.path), m.child_scope()),
        &stored,
    );
    new_module.mark_satisfies(s.sig_id());
    let kt_ref = crate::try_action!(ctx
        .scope
        .alloc_ktype_reaching(KType::Module { module: new_module }, &stored));
    Action::Done(Ok(StepCarried::born(
        ctx.scope.resident_type_carrier(kt_ref, stored),
    )))
}

/// Read the `m:Module` / `s:Signature` operands from the `BodyCtx::args` type channel, producing
/// a missing / mismatch diagnostic when an operand is absent or the wrong kind.
fn resolve_module_and_signature<'a>(
    args: &crate::machine::model::KObject<'a>,
) -> Result<
    (
        &'a crate::machine::model::values::Module<'a>,
        &'a crate::machine::model::values::ModuleSignature<'a>,
    ),
    KError,
> {
    use crate::machine::core::kfunction::action::{arg_held, arg_type};

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

    let m = match arg_type(args, "m") {
        Some(KType::Module { module, .. }) => *module,
        _ => return Err(type_mismatch_or_missing(args, "m", "Module")),
    };
    let s = match arg_type(args, "s") {
        Some(KType::Signature { sig, .. }) => *sig,
        _ => return Err(type_mismatch_or_missing(args, "s", "Signature")),
    };
    Ok((m, s))
}

/// Verify a module supplies everything `sig` declares. Three checks run against `src_scope`,
/// the module's child scope:
///
/// - Every abstract member (`TYPE Elt`, `TYPE (Type AS Wrap)`) must be *present* in the
///   module's type table, bound to any type — abstract members are unconstrained in type, so
///   presence alone satisfies them.
/// - Every manifest member (`LET Tag = Number`) must be present in the type table *and* its
///   type must equal the type the signature fixes (`KType` equality).
/// - Every value slot (`VAL`, value-class name) must have a binding in the module's value
///   table.
fn shape_check<'a>(
    sig: &crate::machine::model::values::ModuleSignature<'a>,
    src_scope: &Scope<'a>,
) -> Result<(), KError> {
    let src_bindings = src_scope.bindings();
    let lookup_type_member = |name: &str| -> Option<KType<'a>> {
        src_bindings
            .lookup_type(name, None)
            .and_then(NameLookup::bound)
            .cloned()
    };

    // Abstract members: presence only (bound to any type).
    for name in abstract_members_of(sig.decl_scope()) {
        if lookup_type_member(&name).is_none() {
            return Err(KError::new(KErrorKind::ShapeError(format!(
                "module does not satisfy signature `{}`: missing type member `{}`",
                sig.path, name
            ))));
        }
    }

    // Manifest members: presence plus `KType` equality with the type the signature fixes.
    for (name, expected) in manifest_type_members_of(sig.decl_scope()) {
        let Some(got) = lookup_type_member(&name) else {
            return Err(KError::new(KErrorKind::ShapeError(format!(
                "module does not satisfy signature `{}`: missing type member `{}`",
                sig.path, name
            ))));
        };
        if got != expected {
            return Err(KError::new(KErrorKind::ShapeError(format!(
                "module does not satisfy signature `{}`: type member `{}` is `{}` but the \
                 signature fixes it to `{}`",
                sig.path,
                name,
                got.render(),
                expected.render()
            ))));
        }
    }

    // A SIG type-table entry is either a value slot (`VAL`, value-class name) or a type
    // member (handled above, type-class names). A satisfying module supplies value slots as
    // values, so the check looks for each value-slot name in the source's value table.
    let src_names: std::collections::HashSet<String> = src_scope
        .bindings()
        .iter_data()
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    for (name, _) in sig.decl_scope().bindings().iter_types() {
        if crate::parse::is_type_name(&name) {
            continue;
        }
        if !src_names.contains(&name) {
            return Err(KError::new(KErrorKind::ShapeError(format!(
                "module does not satisfy signature `{}`: missing member `{}`",
                sig.path, name
            ))));
        }
    }
    Ok(())
}

/// Classify a SIG type-table entry by its *representation*: an abstract member carries no
/// concrete witness. Two abstract shapes — a `Sig`-sourced [`KType::AbstractType`] (the
/// first-order `TYPE Elt` slot) and a sentinel [`KKind::TypeConstructor`] `SetRef` (the
/// higher-kinded `TYPE (Type AS Wrap)` slot, `ScopeId::SENTINEL` marking it "awaiting per-call
/// mint"). Everything else — a manifest `LET Tag = Number` binding a concrete type, a real
/// minted constructor — is manifest.
pub(super) fn is_abstract_sig_member(kt: &KType<'_>) -> bool {
    match kt {
        KType::AbstractType {
            source: AbstractSource::Sig(_),
            ..
        } => true,
        KType::SetRef { set, index } => {
            let member = set.member(*index);
            member.kind == KKind::TypeConstructor && member.scope_id == ScopeId::SENTINEL
        }
        _ => false,
    }
}

/// Type-class-named type-table entries that classify abstract by [`is_abstract_sig_member`].
/// Value slots (`VAL …`, value-class names) filter out by name class.
pub(super) fn abstract_members_of<'a>(scope: &crate::machine::Scope<'a>) -> Vec<String> {
    scope
        .bindings()
        .iter_types()
        .into_iter()
        .filter(|(n, kt)| crate::parse::is_type_name(n) && is_abstract_sig_member(kt))
        .map(|(n, _)| n)
        .collect()
}

/// Type-class-named type-table entries that classify manifest (the concrete witness a
/// satisfying module must match), paired with their fixed `KType`.
pub(super) fn manifest_type_members_of<'a>(
    scope: &crate::machine::Scope<'a>,
) -> Vec<(String, KType<'a>)> {
    scope
        .bindings()
        .iter_types()
        .into_iter()
        .filter(|(n, kt)| crate::parse::is_type_name(n) && !is_abstract_sig_member(kt))
        .map(|(n, kt)| (n, kt.clone()))
        .collect()
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    // Slots are typed `Module` / `Signature`. Bare Type-token operands
    // (`IntOrd :| OrderedSig`) ride the auto-wrap rails into a value-typed future, so
    // no parallel Type-Type overload is required.
    let opaque_sig = sig(
        KType::OfKind(KKind::Module),
        vec![
            arg("m", KType::OfKind(KKind::Module)),
            kw(":|"),
            arg("s", KType::OfKind(KKind::Signature)),
        ],
    );
    let transparent_sig = sig(
        KType::OfKind(KKind::Module),
        vec![
            arg("m", KType::OfKind(KKind::Module)),
            kw(":!"),
            arg("s", KType::OfKind(KKind::Signature)),
        ],
    );
    crate::builtins::register_builtin(scope, ":|", opaque_sig, body_opaque);
    crate::builtins::register_builtin(scope, ":!", transparent_sig, body_transparent);
}

#[cfg(test)]
mod tests;
