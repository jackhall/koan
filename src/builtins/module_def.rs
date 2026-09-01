//! `MODULE <name:Identifier> = <body:KExpression>` — declare a structure (a bundle of
//! type definitions, values, and functions). A module is a value, so it binds value-side under a
//! snake_case name; a second overload takes the Type-token name and reports the respelling. See
//! [design/typing/modules.md](../../design/typing/modules.md) for the surface design.
//!
//! [`await_module_body`] is the body-dispatch-and-bind tail, shared with `GROUP`
//! ([`super::group_def`]) — a group *is* a module, so it differs only in the child scope it mints.

use crate::machine::BindingIndex;
use crate::machine::StepCarried;
use crate::machine::WriteGate;
use crate::machine::core::bindings::WriteOp;
use crate::machine::model::KExpression;
use crate::machine::model::KType;
use crate::machine::model::ValueSymbol;
use crate::machine::model::announce_type_members;
use crate::machine::model::{KKind, SigSchema};
use crate::machine::model::{Module, ModuleDraft};
use crate::machine::{Action, BodyCtx};
use crate::machine::{KError, KErrorKind};
use crate::machine::{NameLookup, Scope};

use super::{arg, kw, sig};
use crate::machine::model::RunRegistries;
use crate::machine::model::render_label;

// This builtin's slot spellings, minted once and read back by symbol.
crate::slots! { SLOTS { body, name } }

/// The MODULE body: pre-announces the body's top-level type declarations, mints the child scope
/// carrying that window, and hands it to [`await_module_body`], which dispatches the body block
/// against it and binds the module **value** into the parent scope's `data`.
pub fn body<'a>(ctx: &BodyCtx<'_, 'a, '_>) -> Action<'a> {
    use crate::machine::{require_identifier_name, require_kexpression};

    let name = crate::try_action!(require_identifier_name(
        ctx.args,
        &SLOTS.name,
        "MODULE",
        ctx.registries
    ));
    let body_expr = crate::try_action!(require_kexpression(ctx.args, "MODULE", &SLOTS.body));
    // The pre-scan quotes the module by name only on the arm that fails, so the binder's symbol
    // carries through to it unrendered and the spelling is written straight into the diagnostic's
    // buffer there. The success path — every `MODULE` that declares — renders nothing.
    let announced = crate::try_action!(announce_type_members(&body_expr, name, ctx.registries));
    let child_scope = ctx.scope.alloc_child_under_module(announced);
    await_module_body(child_scope, name, body_expr, ctx.bind_index())
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
    name: ValueSymbol,
    body_expr: KExpression<'a>,
    bind_index: BindingIndex,
) -> Action<'a> {
    use super::await_body::await_body_in_scope;

    await_body_in_scope(child_scope, body_expr, move |fctx| {
        // A module is a value, so its name binds under a value token — the one the parse classified
        // and interned, which the finish reads as symbol bits throughout.
        let binder = name;
        // Idempotent-finalize guard: a re-bound name short-circuits, re-surfacing the
        // already-bound module value from its **stored** reach.
        if let Some(NameLookup::Bound(sealed)) = fctx.scope.bindings().lookup_value(binder, None) {
            return Action::done(Ok(StepCarried::born_delivered(
                fctx.scope.lift_resident(sealed),
            )));
        }
        if let Some(error) = unsealed_announcement_error(child_scope, name, fctx.registries) {
            return Action::done(Err(error));
        }
        // Mirror the module's type members into the draft. The classified key types keep `data`
        // and `types` disjoint by name, so this is an exact mirror of `iter_types` (no
        // value-member name can also be a type name to filter out). A nested `MODULE` is a
        // value member, so it lives in the child's `data` and is typed by its own self-sig.
        let mut draft = ModuleDraft::empty();
        for (member, kt) in child_scope.bindings().iter_types() {
            draft.type_members.insert(member, kt);
        }
        // The self-sig is derived from the draft and interned before the module exists — a
        // plain module carries no SIG, so the raw derivation is the whole signature.
        let self_sig = fctx
            .types()
            .signature(SigSchema::raw_self_sig(child_scope, &draft));
        // A module's path is its surface spelling, so the binder's symbol renders once here — the
        // one text this finish needs.
        let path = render_label(name.symbol(), fctx.registries);
        let module: &'a Module<'a> =
            Module::alloc_at_child_scope(&path, child_scope, draft, self_sig);
        // Fused MODULE-finish seal: the module reference held **directly** here (never
        // recovered by walking the built value) is merged into this scope's region, which mints
        // and retains the child's region as the Object-arm module value's reach — this scope's
        // own region, for a same-region child. The returned terminal witnesses that same value
        // from the same composed reach; the value-side (`bindings.data`) write rides the outcome.
        let sealed = fctx.scope.seal_module(module);
        let write = WriteOp::Value {
            name: binder,
            index: bind_index,
            sealed: sealed.duplicate(),
        };
        Action::done(Ok(StepCarried::born_delivered(
            fctx.scope.lift_resident(sealed),
        )))
        .with_effect(fctx.scratch, write)
    })
}

/// The belt check on the announcement contract: every announced member's declaration ran and
/// filled its slot, so the group sealed. `None` is the ordinary outcome — a body statement that
/// errored already errored the module before this, so a `Some` here means the scan and dispatch
/// disagreed about what a statement declares, which is a wiring bug surfaced as a typed error
/// rather than a module that binds half a group.
pub(super) fn unsealed_announcement_error(
    child_scope: &Scope<'_>,
    name: ValueSymbol,
    registries: &RunRegistries,
) -> Option<KError> {
    let window = child_scope.own_declaration_window()?;
    if window.is_sealed() {
        return None;
    }
    let name = render_label(name.symbol(), registries);
    // A variant carries no declaration of its own: its binder's statement is what never filled it.
    let unfilled: Vec<String> = window
        .unfilled_members()
        .into_iter()
        .map(|(member, owner)| render_label(owner.unwrap_or(member).symbol(), registries))
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
/// value, so its name belongs in the value namespace. It always errors, so it installs nothing.
pub(super) fn body_type_named<'a>(ctx: &BodyCtx<'_, 'a, '_>) -> Action<'a> {
    use crate::machine::require_bare_type_name;
    use crate::machine::{KError, KErrorKind};

    let name = crate::try_action!(require_bare_type_name(
        ctx.args,
        &SLOTS.name,
        "MODULE",
        ctx.registries
    ));
    let name = crate::machine::model::render_label(name.symbol(), ctx.registries);
    Action::done(Err(KError::new(KErrorKind::ShapeError(format!(
        "module `{name}` is named with a Type token, but a module is a value — the Type-token \
         namespace names what can type a field. Name it snake_case, e.g. `{suggestion}`",
        suggestion = super::let_binding::snake_case_identifier(&name),
    )))))
}

pub fn register<'a>(scope: &'a Scope<'a>, registries: &RunRegistries, gate: &mut WriteGate) {
    let module_sig = |name_kt: KType| {
        sig(
            KType::EMPTY_SIGNATURE,
            vec![
                kw(registries, "MODULE"),
                arg(registries, &SLOTS.name, name_kt),
                kw(registries, "="),
                arg(registries, &SLOTS.body, KType::KEXPRESSION),
            ],
        )
    };
    crate::builtins::register_builtin(scope, module_sig(KType::IDENTIFIER), body, registries, gate);
    crate::builtins::register_builtin(
        scope,
        module_sig(KType::of_kind(KKind::ProperType)),
        body_type_named,
        registries,
        gate,
    );
}

#[cfg(test)]
mod tests;
