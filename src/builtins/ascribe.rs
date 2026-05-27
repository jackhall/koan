//! Ascription operators `:|` (opaque) and `:!` (transparent) — bolt a [`Signature`] onto
//! a [`Module`]. Both consume `(Module, Signature)` and produce a `Module`.
//! See [design/typing/modules.md](../../design/typing/modules.md).
//!
//! Today's shape-checking is name-presence only; full type-shape checks are deferred to
//! the inference scheduler.

use crate::machine::model::{KObject, KType};
use crate::machine::model::types::UserTypeKind;
use crate::machine::{ArgumentBundle, BodyResult, KError, KErrorKind, Scope, SchedulerHandle};
use crate::machine::model::values::Module;

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

    // Mirror the source module's bindings into the new scope by reference (values are
    // arena-allocated and immutable). The `try_bulk_install_from` helper snapshots
    // `src.data`, releases the source guard, and replays each entry through the shared
    // `try_apply` so the `KFunction → functions` mirror happens exactly once per entry
    // — no separate functions-loop needed.
    let src = m.child_scope();
    if let Err(e) = new_scope.bindings().try_bulk_install_from(src.bindings()) {
        return BodyResult::Err(e);
    }

    let new_module: &'a Module<'a> = arena.alloc_module(Module::new(m.path.clone(), new_scope));
    // Each minted abstract type carries the new module's `&Module` pointer; manual
    // `PartialEq` on `KType::AbstractType` compares `(source_module.scope_id(), name)`
    // so two opaque ascriptions of the same source-and-name compare equal even though
    // each ascription allocates a fresh `&Module` (with a distinct `scope_id`,
    // preserving the abstraction barrier between two ascriptions).
    //
    // Per-slot kind selection. A SIG slot declared with `LET Wrap = (TYPE_CONSTRUCTOR T)`
    // lives in the SIG's decl_scope as a `KType::UserType { kind: TypeConstructor
    // { param_names }, .. }` template; mint a fresh per-call `TypeConstructor` rather
    // than the default `AbstractType` arm. The lookup inspects `bindings.types`
    // (where Type-class LET aliases land via `register_type`) and falls back to the
    // default `AbstractType` mint for plain abstract-type slots (`LET Type = Number`).
    let mut minted: Vec<(String, KType<'a>)> = Vec::new();
    {
        let sig_bindings = s.decl_scope().bindings();
        for name in abstract_type_names_of(s.decl_scope()) {
            let kt = match sig_bindings.lookup_type(&name, None) {
                Some(KType::UserType { kind: UserTypeKind::TypeConstructor { param_names }, .. }) => {
                    KType::UserType {
                        kind: UserTypeKind::TypeConstructor { param_names: param_names.clone() },
                        scope_id: new_module.scope_id(),
                        name: name.clone(),
                    }
                }
                _ => KType::AbstractType {
                    source_module: new_module,
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

    if let Err(e) = shape_check(s, src) {
        return BodyResult::Err(e);
    }

    // Record the sig in the new module's compat set so a `KType::SatisfiesSignature { sig_id }`
    // slot accepts this module. Every ascription path must do this — see
    // `Module::mark_satisfies` for the bookkeeping discipline.
    new_module.mark_satisfies(s.sig_id());

    // Ascription paths run on the outer scheduler; the resulting `Module` lives in `arena`
    // (the calling scope's arena), not in any per-call frame. `frame: None` is correct.
    let module_obj: &'a KObject<'a> = arena.alloc(KObject::KTypeValue(
        KType::Module { module: new_module, frame: None },
    ));
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
    // Same compat-set bookkeeping as `body_opaque`. `:!` makes the module appear as the
    // sig at the type level too — sig-typed slots accept it.
    new_module.mark_satisfies(s.sig_id());
    let module_obj: &'a KObject<'a> = arena.alloc(KObject::KTypeValue(
        KType::Module { module: new_module, frame: None },
    ));
    BodyResult::Value(module_obj)
}

/// Verify every non-abstract-type name in `sig` has a binding in `src_scope`.
/// Abstract-type declarations are skipped: they shape the abstraction, not the implementation.
fn shape_check<'a>(
    sig: &crate::machine::model::values::Signature<'a>,
    src_scope: &Scope<'a>,
) -> Result<(), KError> {
    let abstract_names: std::collections::HashSet<String> =
        abstract_type_names_of(sig.decl_scope()).into_iter().collect();
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
/// Type-class LET aliases write `bindings.types` via `register_type`; other carriers that
/// classify as Type-tokens at use still land on `bindings.data`. Sweeping both maps keeps
/// the helper's answer robust to either binding home; names already in `types` are not
/// duplicated.
///
/// Goes through the [`Bindings`](crate::machine::core::Bindings) façade via the
/// value-yielding `iter_types` / `iter_data` helpers — no raw `RefCell`
/// reach-around — so the underlying borrows release at the iterator boundary.
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
    let Some(first) = chars.next() else { return false; };
    if !first.is_ascii_uppercase() {
        return false;
    }
    chars.any(|c| c.is_ascii_lowercase())
}

/// Resolve `m` and `s` from the bundle. Both slots are typed `Module` / `Signature`, so
/// the resolver is just a typed `require_module()` / `require_signature()` projection; the
/// `TypeMismatch` arm is a defensive guard against a future caller wiring something else.
fn resolve_module_and_signature<'a>(
    bundle: &ArgumentBundle<'a>,
) -> Result<(&'a crate::machine::model::values::Module<'a>, &'a crate::machine::model::values::Signature<'a>), KError> {
    let m = bundle.require_module("m")?;
    let s = bundle.require_signature("s")?;
    Ok((m, s))
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    // Both ascription operators take already-evaluated `Module` / `Signature` values.
    // Bare Type-token operands (`IntOrd :| OrderedSig`) ride the unified auto-wrap +
    // replay-park rails in [`KFunction::classify_for_pick`] — they sub-dispatch through
    // the `value_lookup`-TypeExprRef overload to a
    // `Future(KTypeValue(KType::Module/Signature))` which then matches these slots
    // strictly. No parallel Type-Type overload required.
    register_builtin(
        scope,
        ":|",
        sig(KType::AnyModule, vec![
            arg("m", KType::AnyModule),
            kw(":|"),
            arg("s", KType::AnySignature),
        ]),
        body_opaque,
    );
    register_builtin(
        scope,
        ":!",
        sig(KType::AnyModule, vec![
            arg("m", KType::AnyModule),
            kw(":!"),
            arg("s", KType::AnySignature),
        ]),
        body_transparent,
    );
}

#[cfg(test)]
mod tests;
