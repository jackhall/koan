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
use crate::machine::{ArgumentBundle, BodyResult, KError, KErrorKind, SchedulerHandle, Scope};

use super::{arg, kw, sig};
#[cfg(not(feature = "action-harness"))]
use super::register_builtin;

/// `<m:Module> :| <s:Signature>` — opaque ascription.
pub fn body_opaque<'a, 's>(
    sched: &mut dyn SchedulerHandle<'a, 's>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let (m, s) = match resolve_module_and_signature(&bundle) {
        Ok(pair) => pair,
        Err(e) => return BodyResult::Err(e),
    };

    let arena = sched.current_scope().arena;
    let new_scope = arena.alloc_scope(Scope::child_under_module(
        sched.current_scope(),
        format!("{} :| {}", m.path, s.path),
    ));

    let src = m.child_scope();
    if let Err(e) = new_scope.bindings().try_bulk_install_from(src.bindings()) {
        return BodyResult::Err(e);
    }

    let new_module: &'a Module<'a> = arena.alloc_module(Module::new(m.path.clone(), new_scope));
    // Per-slot kind: a SIG-declared `LET Wrap = (TEMPLATE T)` mints a fresh
    // `TypeConstructor` rather than the default `AbstractType` arm, preserving the
    // higher-kinded shape across the ascription barrier.
    let mut minted: Vec<(String, KType<'a>)> = Vec::new();
    {
        let sig_bindings = s.decl_scope().bindings();
        for name in abstract_type_names_of(s.decl_scope()) {
            // A SIG-declared higher-kinded `LET Wrap = (TEMPLATE T)` is a singleton
            // `TypeConstructor` set. Re-mint a fresh per-call singleton (new scope_id, same
            // schema + param_names) so the higher-kinded shape survives the ascription
            // barrier; every other slot collapses to the `AbstractType` arm.
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

    // Thread per-call slot tags: a VAL slot whose SIG-declared type is a `Sig`-rooted
    // abstract member (`VAL zero :Carrier` where `Carrier` is a SIG-local `LET Carrier = ...`) is
    // tagged with the per-call `type_members[member]` identity. ATTR re-tags the slot read
    // with this identity so `(int_ord.zero)` reads as the abstract `Carrier`, not the
    // underlying value. Structural-form slot types (`:(FN (Type, Type) -> Number)`) are
    // out of scope — only a bare `Sig`-rooted member naming a minted type is tagged.
    {
        let tm = new_module.type_members.borrow();
        let mut tags: Vec<(String, KType<'a>)> = Vec::new();
        for (slot_name, kt) in s.decl_scope().bindings().iter_types() {
            // Only value-slot (VAL) entries carry a slot tag; the abstract-type members
            // themselves are Type-class names read type-side, not value-side slots.
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
        return BodyResult::Err(e);
    }

    new_module.mark_satisfies(s.sig_id());

    let module_obj: &'a KType<'a> = arena.alloc_ktype(KType::Module {
        module: new_module,
        frame: None,
    });
    BodyResult::ktype(module_obj)
}

/// `<m:Module> :! <s:Signature>` — transparent ascription.
pub fn body_transparent<'a, 's>(
    sched: &mut dyn SchedulerHandle<'a, 's>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let (m, s) = match resolve_module_and_signature(&bundle) {
        Ok(pair) => pair,
        Err(e) => return BodyResult::Err(e),
    };
    if let Err(e) = shape_check(s, m.child_scope()) {
        return BodyResult::Err(e);
    }
    // Reuse the source's child scope; the new Module just retags the path as a view.
    let arena = sched.current_scope().arena;
    let new_module: &'a Module<'a> = arena.alloc_module(Module::new(
        format!("{} :! {}", m.path, s.path),
        m.child_scope(),
    ));
    new_module.mark_satisfies(s.sig_id());
    let module_obj: &'a KType<'a> = arena.alloc_ktype(KType::Module {
        module: new_module,
        frame: None,
    });
    BodyResult::ktype(module_obj)
}

/// `Action`-harness twin of [`body_opaque`]: same opaque-ascription logic, reading `m` / `s` from
/// the `BodyCtx::args` type channel, minting on `ctx.scope.arena`, and returning the view module as
/// `Action::Done(Ok(Carried::Type(..)))`.
#[cfg(feature = "action-harness")]
pub fn body_opaque_action<'a>(
    ctx: &crate::machine::core::kfunction::action::BodyCtx<'a, '_>,
) -> crate::machine::core::kfunction::action::Action<'a> {
    use crate::machine::core::kfunction::action::Action;
    use crate::machine::model::Carried;

    let (m, s) = match resolve_module_and_signature_action(ctx.args) {
        Ok(pair) => pair,
        Err(e) => return Action::Done(Err(e)),
    };

    let arena = ctx.scope.arena;
    let new_scope = arena.alloc_scope(Scope::child_under_module(
        ctx.scope,
        format!("{} :| {}", m.path, s.path),
    ));

    let src = m.child_scope();
    if let Err(e) = new_scope.bindings().try_bulk_install_from(src.bindings()) {
        return Action::Done(Err(e));
    }

    let new_module: &'a Module<'a> = arena.alloc_module(Module::new(m.path.clone(), new_scope));
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

    let module_obj: &'a KType<'a> = arena.alloc_ktype(KType::Module {
        module: new_module,
        frame: None,
    });
    Action::Done(Ok(Carried::Type(module_obj)))
}

/// `Action`-harness twin of [`body_transparent`]: shape-checks against the source's own child scope
/// and returns the retagged view module as `Action::Done(Ok(Carried::Type(..)))`.
#[cfg(feature = "action-harness")]
pub fn body_transparent_action<'a>(
    ctx: &crate::machine::core::kfunction::action::BodyCtx<'a, '_>,
) -> crate::machine::core::kfunction::action::Action<'a> {
    use crate::machine::core::kfunction::action::Action;
    use crate::machine::model::Carried;

    let (m, s) = match resolve_module_and_signature_action(ctx.args) {
        Ok(pair) => pair,
        Err(e) => return Action::Done(Err(e)),
    };
    if let Err(e) = shape_check(s, m.child_scope()) {
        return Action::Done(Err(e));
    }
    let arena = ctx.scope.arena;
    let new_module: &'a Module<'a> = arena.alloc_module(Module::new(
        format!("{} :! {}", m.path, s.path),
        m.child_scope(),
    ));
    new_module.mark_satisfies(s.sig_id());
    let module_obj: &'a KType<'a> = arena.alloc_ktype(KType::Module {
        module: new_module,
        frame: None,
    });
    Action::Done(Ok(Carried::Type(module_obj)))
}

/// `Action`-side resolver: read the `m:Module` / `s:Signature` operands from the `BodyCtx::args`
/// type channel, reproducing [`resolve_module_and_signature`]'s missing / mismatch diagnostics.
#[cfg(feature = "action-harness")]
fn resolve_module_and_signature_action<'a>(
    args: &crate::machine::model::KObject<'a>,
) -> Result<
    (
        &'a crate::machine::model::values::Module<'a>,
        &'a crate::machine::model::values::Signature<'a>,
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
    sig: &crate::machine::model::values::Signature<'a>,
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
/// (`LET <TypeName> = …`) under Type-class names and value slots (`VAL …`) under value-class
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

fn resolve_module_and_signature<'a>(
    bundle: &ArgumentBundle<'a>,
) -> Result<
    (
        &'a crate::machine::model::values::Module<'a>,
        &'a crate::machine::model::values::Signature<'a>,
    ),
    KError,
> {
    let m = bundle.require_module("m")?;
    let s = bundle.require_signature("s")?;
    Ok((m, s))
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
    #[cfg(feature = "action-harness")]
    {
        crate::builtins::register_action_builtin(scope, ":|", opaque_sig, body_opaque_action);
        crate::builtins::register_action_builtin(
            scope,
            ":!",
            transparent_sig,
            body_transparent_action,
        );
    }
    #[cfg(not(feature = "action-harness"))]
    {
        register_builtin(scope, ":|", opaque_sig, body_opaque);
        register_builtin(scope, ":!", transparent_sig, body_transparent);
    }
}

#[cfg(test)]
mod tests;
