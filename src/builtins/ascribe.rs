//! Ascription operators `:|` (opaque) and `:!` (transparent).
//! See [design/typing/modules.md](../../design/typing/modules.md).
//!
//! Shape-checking is name-presence only; full type-shape checks are deferred to
//! the inference scheduler.

use crate::machine::model::types::{
    AbstractSource, KKind, NominalMember, NominalSchema, ProjectedSchema, RecursiveSet,
};
use crate::machine::model::values::Module;
use crate::machine::model::KType;
use crate::machine::{KError, KErrorKind, Scope};

use super::{arg, kw, sig};

/// `<m:Module> :| <s:Signature>` — opaque ascription. Reads `m` / `s` from the
/// `BodyCtx::args` type channel, mints on `ctx.scope.region`, and returns the view module as a
/// witnessed [`Action::DoneWitnessed`] carrier (`Scope::seal_module` folds the child scope's reach).
pub fn body_opaque<'a>(
    ctx: &crate::machine::core::kfunction::action::BodyCtx<'a, '_>,
) -> crate::machine::core::kfunction::action::Action<'a> {
    use crate::machine::core::kfunction::action::Action;
    use crate::machine::model::Carried;

    let (m, s) = crate::try_action!(resolve_module_and_signature(ctx.args));

    let region = ctx.scope.region;
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
    // Per-slot kind: a SIG-declared `LET Wrap = (TEMPLATE T)` mints a fresh
    // `TypeConstructor` rather than the default `AbstractType` arm, preserving the
    // higher-kinded shape across the ascription barrier.
    let mut minted: Vec<(String, KType<'a>)> = Vec::new();
    {
        let sig_bindings = s.decl_scope().bindings();
        for name in abstract_type_names_of(s.decl_scope()) {
            let kt = match sig_bindings.lookup_type(&name, None) {
                Some(KType::SetRef { set, index })
                    if set.member(*index).kind == KKind::TypeConstructor =>
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
    if !minted.is_empty() {
        let mut tm = new_module.type_members.borrow_mut();
        for (n, t) in minted {
            tm.insert(n, t);
        }
    }

    {
        let tm = new_module.type_members.borrow();
        let mut tags: Vec<(String, KType<'a>)> = Vec::new();
        for (slot_name, kt) in s.decl_scope().bindings().iter_types() {
            if is_abstract_type_name(&slot_name) {
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

    let module_obj: &'a KType<'a> = region.alloc_ktype(KType::Module { module: new_module });
    Action::DoneWitnessed(ctx.scope.seal_module(Carried::Type(module_obj)))
}

/// `<m:Module> :! <s:Signature>` — transparent ascription. Shape-checks against the source's
/// own child scope and returns the retagged view module as a witnessed [`Action::DoneWitnessed`]
/// carrier — `seal_module` pins the (foreign) source module's child-scope region the view borrows.
pub fn body_transparent<'a>(
    ctx: &crate::machine::core::kfunction::action::BodyCtx<'a, '_>,
) -> crate::machine::core::kfunction::action::Action<'a> {
    use crate::machine::core::kfunction::action::Action;
    use crate::machine::model::Carried;

    let (m, s) = crate::try_action!(resolve_module_and_signature(ctx.args));
    if let Err(e) = shape_check(s, m.child_scope()) {
        return Action::Done(Err(e));
    }
    let region = ctx.scope.region;
    let new_module: &'a Module<'a> = region.alloc_module(Module::new(
        format!("{} :! {}", m.path, s.path),
        m.child_scope(),
    ));
    new_module.mark_satisfies(s.sig_id());
    let module_obj: &'a KType<'a> = region.alloc_ktype(KType::Module { module: new_module });
    Action::DoneWitnessed(ctx.scope.seal_module(Carried::Type(module_obj)))
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

/// Verify every non-abstract-type name in `sig` has a binding in `src_scope`.
fn shape_check<'a>(
    sig: &crate::machine::model::values::ModuleSignature<'a>,
    src_scope: &Scope<'a>,
) -> Result<(), KError> {
    let abstract_names: std::collections::HashSet<String> =
        abstract_type_names_of(sig.decl_scope())
            .into_iter()
            .collect();
    // SIG members all live in the type table: abstract types (skipped below) and VAL value
    // slots — the names a satisfying module must supply. The module supplies them as values,
    // so the satisfaction check looks for each in the source's value table.
    let sig_names: Vec<String> = sig
        .decl_scope()
        .bindings()
        .iter_types()
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    let src_names: std::collections::HashSet<String> = src_scope
        .bindings()
        .iter_data()
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    for name in sig_names {
        if abstract_names.contains(name.as_str()) {
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

/// Collect every name in `scope`'s `Bindings` that classifies as an abstract Type member.
/// Every SIG-body declaration lives in `bindings.types`: abstract-type members
/// (`LET <TypeIdentifier> = …`) under Type-class names and value slots (`VAL …`) under value-class
/// names. An abstract type member is exactly a Type-class-named type-table entry, so the
/// value slots filter out by name class.
pub(super) fn abstract_type_names_of<'a>(scope: &crate::machine::Scope<'a>) -> Vec<String> {
    scope
        .bindings()
        .iter_types()
        .into_iter()
        .map(|(n, _)| n)
        .filter(|n| is_abstract_type_name(n))
        .collect()
}

/// True iff `name` classifies as a Type token (first char uppercase + at least one
/// lowercase elsewhere). See [design/typing/tokens.md](../../design/typing/tokens.md).
pub(super) fn is_abstract_type_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_uppercase() {
        return false;
    }
    chars.any(|c| c.is_ascii_lowercase())
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
