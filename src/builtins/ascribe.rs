//! Ascription operators `:|` (opaque) and `:!` (transparent).
//! See [design/typing/modules.md](../../design/typing/modules.md).
//!
//! Shape-checking is name-presence only; full type-shape checks are deferred to
//! the inference scheduler.

use crate::machine::model::types::{AbstractSource, UserTypeKind};
use crate::machine::model::values::Module;
use crate::machine::model::{KObject, KType};
use crate::machine::{ArgumentBundle, BodyResult, KError, KErrorKind, SchedulerHandle, Scope};

use super::{arg, kw, register_builtin, sig};

/// `<m:Module> :| <s:Signature>` — opaque ascription.
pub fn body_opaque<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let (m, s) = match resolve_module_and_signature(&bundle) {
        Ok(pair) => pair,
        Err(e) => return BodyResult::Err(e),
    };

    let arena = scope.arena;
    let new_scope = arena.alloc_scope(Scope::child_under_module(
        scope,
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
            let kt = match sig_bindings.lookup_type(&name, None) {
                Some(KType::UserType {
                    kind:
                        UserTypeKind::TypeConstructor {
                            schema,
                            param_names,
                        },
                    ..
                }) => KType::UserType {
                    kind: UserTypeKind::TypeConstructor {
                        schema: std::rc::Rc::clone(schema),
                        param_names: param_names.clone(),
                    },
                    scope_id: new_module.scope_id(),
                    name: name.clone(),
                },
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
    // abstract member (`VAL zero :Type` where `Type` is a SIG-local `LET Type = ...`) is
    // tagged with the per-call `type_members[member]` identity. ATTR re-tags the slot read
    // with this identity so `(int_ord.zero)` reads as the abstract `Type`, not the
    // underlying value. Structural-form slot types (`:(FN (Type, Type) -> Number)`) are
    // out of scope — only a bare `Sig`-rooted member naming a minted type is tagged.
    {
        let tm = new_module.type_members.borrow();
        let mut tags: Vec<(String, KType<'a>)> = Vec::new();
        for (slot_name, value) in s.decl_scope().bindings().iter_data() {
            if let KObject::KTypeValue(KType::AbstractType {
                source: AbstractSource::Sig(_),
                name: member,
            }) = value
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

    let module_obj: &'a KObject<'a> = arena.alloc(KObject::KTypeValue(KType::Module {
        module: new_module,
        frame: None,
    }));
    BodyResult::Value(module_obj)
}

/// `<m:Module> :! <s:Signature>` — transparent ascription.
pub fn body_transparent<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
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
    let arena = scope.arena;
    let new_module: &'a Module<'a> = arena.alloc_module(Module::new(
        format!("{} :! {}", m.path, s.path),
        m.child_scope(),
    ));
    new_module.mark_satisfies(s.sig_id());
    let module_obj: &'a KObject<'a> = arena.alloc(KObject::KTypeValue(KType::Module {
        module: new_module,
        frame: None,
    }));
    BodyResult::Value(module_obj)
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
    let sig_names: Vec<String> = sig
        .decl_scope()
        .bindings()
        .iter_data()
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
/// Sweeps both `types` (Type-class LET aliases) and `data` (Type-token carriers) so the
/// answer is robust to either binding home; names already in `types` are not duplicated.
pub(super) fn abstract_type_names_of<'a>(scope: &crate::machine::Scope<'a>) -> Vec<String> {
    let bindings = scope.bindings();
    let mut names: Vec<String> = bindings.iter_types().into_iter().map(|(n, _)| n).collect();
    let types_set: std::collections::HashSet<String> = names.iter().cloned().collect();
    for (name, _) in bindings.iter_data() {
        if is_abstract_type_name(&name) && !types_set.contains(&name) {
            names.push(name);
        }
    }
    names
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
    register_builtin(
        scope,
        ":|",
        sig(
            KType::AnyModule,
            vec![
                arg("m", KType::AnyModule),
                kw(":|"),
                arg("s", KType::AnySignature),
            ],
        ),
        body_opaque,
    );
    register_builtin(
        scope,
        ":!",
        sig(
            KType::AnyModule,
            vec![
                arg("m", KType::AnyModule),
                kw(":!"),
                arg("s", KType::AnySignature),
            ],
        ),
        body_transparent,
    );
}

#[cfg(test)]
mod tests;
