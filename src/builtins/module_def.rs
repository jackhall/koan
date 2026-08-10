//! `MODULE <name:Identifier> = <body:KExpression>` — declare a structure (a bundle of
//! type definitions, values, and functions). A module is a value, so it binds value-side under a
//! snake_case name; a second overload takes the Type-token name and reports the respelling. See
//! [design/typing/modules.md](../../design/typing/modules.md) for the surface design.
//!
//! [`await_module_body`] is the body-dispatch-and-bind tail, shared with `GROUP`
//! ([`super::group_def`]) — a group *is* a module, so it differs only in the child scope it mints.

use crate::machine::body_statement_refs;
use crate::machine::core::bindings::WriteOp;
use crate::machine::model::KExpression;
use crate::machine::model::KType;
use crate::machine::model::TypeRegistry;
use crate::machine::model::{announced_type_declaration, TypeDeclarationSurface};
use crate::machine::model::{pair_list_names, AnnouncedData, FieldNameKind};
use crate::machine::model::{KKind, SigSchema};
use crate::machine::model::{Module, ModuleDraft};
use crate::machine::BindingIndex;
use crate::machine::StepCarried;
use crate::machine::WriteGate;
use crate::machine::{Action, BodyCtx};
use crate::machine::{KError, KErrorKind};
use crate::machine::{NameLookup, Scope};

use super::{arg, kw, sig};

/// The MODULE body: pre-announces the body's top-level type declarations, mints the child scope
/// carrying that window, and hands it to [`await_module_body`], which dispatches the body block
/// against it and binds the module **value** into the parent scope's `data`.
pub fn body<'a>(ctx: &BodyCtx<'_, 'a, '_>) -> Action<'a> {
    use crate::machine::{require_identifier_name, require_kexpression};

    let name = crate::try_action!(require_identifier_name(
        ctx.args, "name", "MODULE", ctx.types
    ));
    let body_expr = crate::try_action!(require_kexpression(ctx.args, "MODULE", "body"));
    let announced = crate::try_action!(announce_type_members(&body_expr, &name));
    let child_scope = ctx.scope.alloc_child_under_module(&name, announced);
    await_module_body(child_scope, name, body_expr, ctx.bind_index())
}

/// Pre-scan `body`'s **top-level** statements for the type declarations the body announces, so
/// every one of their names is visible to every statement regardless of order — which is what lets
/// a plain module host a mutually-recursive group.
///
/// A `NEWTYPE` announces one standalone member. A `UNION` announces one member per statically
/// scannable variant tag, each **owned** by the union's binder: a variant is never
/// bare-name-resolvable and never lands in `bindings.types`, so it is reached only through the
/// binder or the qualified sigil. A `UNION` whose schema does not scan announces nothing at all —
/// its own dispatch surfaces the real diagnostic.
///
/// Nested and computed declarations are untouched by construction: the scan sees only the statement
/// split [`body_statement_refs`] draws, the same boundary `GROUP` reads its members off.
pub(super) fn announce_type_members(
    body: &KExpression<'_>,
    module: &str,
) -> Result<Option<AnnouncedData>, KError> {
    let mut announced = AnnouncedData::default();
    for statement in body_statement_refs(body) {
        let Some(surface) = announced_type_declaration(statement) else {
            continue;
        };
        let Some(name) = statement.binder_name_from_type_part() else {
            continue;
        };
        if announced.declares(name) || announced.binds(name) {
            return Err(KError::new(KErrorKind::ShapeError(format!(
                "module `{module}` declares type `{name}` twice",
            ))));
        }
        match surface {
            TypeDeclarationSurface::NewType => {
                announced.announce(name.to_string());
            }
            TypeDeclarationSurface::Union => {
                // The variant tags are the union's announced members. A schema this scan cannot
                // read is left entirely unannounced rather than half-announced.
                let Some(schema) = union_schema(statement) else {
                    continue;
                };
                match pair_list_names(&schema, "UNION schema", FieldNameKind::Type) {
                    Ok(tags) => announced.announce_binder(name.to_string(), tags),
                    Err(_) => continue,
                }
            }
        }
    }
    Ok((!announced.is_empty()).then_some(announced))
}

/// The schema expression of a `UNION <name> = (<schema>)` statement — its final slot.
fn union_schema<'a>(statement: &KExpression<'a>) -> Option<KExpression<'a>> {
    match statement.parts.last()?.value {
        crate::machine::model::ExpressionPart::Expression(schema) => Some(*schema),
        _ => None,
    }
}

/// Dispatch a module body block against an already-minted `child_scope` and bind the resulting
/// module value in the parent scope — the tail every module-shaped declaration shares. `GROUP`
/// (`super::group_def`) mints its child through [`Scope::alloc_child_under_group`] and pre-registers
/// the group's operator powerset into it before calling this; `MODULE` mints a plain
/// [`Scope::alloc_child_under_module`].
///
/// Body statements dispatch on the OUTER scheduler (see
/// [`await_body_in_scope`](super::await_body::await_body_in_scope)), so a body statement
/// referencing an earlier sibling at the same outer block parks on the outer placeholder like any
/// other forward reference, and the parent binding lands at dep-finish, not when the declaration's
/// body returns to the dispatcher.
pub(super) fn await_module_body<'a>(
    child_scope: &'a Scope<'a>,
    name: String,
    body_expr: KExpression<'a>,
    bind_index: BindingIndex,
) -> Action<'a> {
    use super::await_body::await_body_in_scope;

    let name_for_finish = name;
    await_body_in_scope(child_scope, body_expr, move |fctx| {
        // Idempotent-finalize guard: a re-bound name short-circuits, re-surfacing the
        // already-bound module value from its **stored** reach.
        if let Some(NameLookup::Bound(sealed)) =
            fctx.scope.bindings().lookup_value(&name_for_finish, None)
        {
            return Action::done(Ok(StepCarried::born_delivered(
                fctx.scope.lift_resident(sealed),
            )));
        }
        if let Some(error) = unsealed_announcement_error(child_scope, &name_for_finish) {
            return Action::done(Err(error));
        }
        // Mirror the module's type members into the draft. The cross-kind exclusion keeps
        // `data` and `types` disjoint by name, so this is an exact mirror of `iter_types` (no
        // value-member name can also be a type name to filter out). A nested `MODULE` is a
        // value member, so it lives in the child's `data` and is typed by its own self-sig.
        let mut draft = ModuleDraft::empty();
        for (member, kt) in child_scope.bindings().iter_types() {
            draft.type_members.insert(member, kt);
        }
        // The self-sig is derived from the draft and interned before the module exists — a
        // plain module carries no SIG, so the raw derivation is the whole signature.
        let self_sig = fctx
            .types
            .signature(SigSchema::raw_self_sig(child_scope, &draft));
        let module: &'a Module<'a> =
            Module::alloc_at_child_scope(&name_for_finish, child_scope, draft, self_sig);
        // Fused MODULE-finish seal: the module reference held **directly** here (never
        // recovered by walking the built value) is merged into this scope's region, which mints
        // and retains the child's region as the Object-arm module value's reach — this scope's
        // own region, for a same-region child. The returned terminal witnesses that same value
        // from the same composed reach; the value-side (`bindings.data`) write rides the outcome.
        let sealed = fctx.scope.seal_module(module);
        let write = WriteOp::Value {
            name: name_for_finish,
            index: bind_index,
            sealed: sealed.duplicate(),
        };
        Action::done(Ok(StepCarried::born_delivered(
            fctx.scope.lift_resident(sealed),
        )))
        .with_effect(write)
    })
}

/// The belt check on the announcement contract: every announced member's declaration ran and
/// filled its slot, so the group sealed. `None` is the ordinary outcome — a body statement that
/// errored already errored the module before this, so a `Some` here means the scan and dispatch
/// disagreed about what a statement declares, which is a wiring bug surfaced as a typed error
/// rather than a module that binds half a group.
pub(super) fn unsealed_announcement_error(child_scope: &Scope<'_>, name: &str) -> Option<KError> {
    let window = child_scope.own_declaration_window()?;
    if window.is_sealed() {
        return None;
    }
    // A variant carries no declaration of its own: its binder's statement is what never filled it.
    let unfilled: Vec<&str> = window
        .unfilled_members()
        .into_iter()
        .map(|(member, owner)| owner.unwrap_or(member))
        .collect();
    Some(
        KError::new(KErrorKind::ShapeError(format!(
            "module `{name}` announced type `{}` but its declaration never sealed",
            unfilled.join("`, `"),
        )))
        .with_frame(crate::machine::TraceFrame::bare(
            "<module>",
            format!("MODULE {name}"),
        )),
    )
}

/// The Type-token-named overload (`MODULE IntOrd = …`, `GROUP VecOps FOLD LEFT = …`): a module is a
/// value, so its name belongs in the value namespace. Registered with no binder hook — it always
/// errors, so it installs nothing.
pub(super) fn body_type_named<'a>(ctx: &BodyCtx<'_, 'a, '_>) -> Action<'a> {
    use crate::machine::require_bare_type_name;
    use crate::machine::{KError, KErrorKind};

    let name = crate::try_action!(require_bare_type_name(
        ctx.args, "name", "MODULE", ctx.types
    ));
    Action::done(Err(KError::new(KErrorKind::ShapeError(format!(
        "module `{name}` is named with a Type token, but a module is a value — the Type-token \
         namespace names what can type a field. Name it snake_case, e.g. `{suggestion}`",
        suggestion = super::let_binding::snake_case_identifier(&name),
    )))))
}

pub fn register<'a>(scope: &'a Scope<'a>, types: &TypeRegistry, gate: &mut WriteGate) {
    let module_sig = |name_kt: KType| {
        sig(
            KType::EMPTY_SIGNATURE,
            vec![
                kw("MODULE"),
                arg("name", name_kt),
                kw("="),
                arg("body", KType::KEXPRESSION),
            ],
        )
    };
    crate::builtins::register_builtin_full(
        scope,
        "MODULE",
        module_sig(KType::IDENTIFIER),
        body,
        true,
        types,
        gate,
    );
    crate::builtins::register_builtin_full(
        scope,
        "MODULE",
        module_sig(KType::of_kind(KKind::ProperType)),
        body_type_named,
        false,
        types,
        gate,
    );
}

#[cfg(test)]
mod tests;
