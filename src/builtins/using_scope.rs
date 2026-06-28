//! `USING <module> SCOPE <block>` — block-scoped module opening. See
//! `design/typing/modules.md` § "Block-scoped opening".
//!
//! `m` is eager (a resolved module value), `body` is lazy
//! ([`KType::KExpression`]) so it evaluates in the opened scope.
//!
//! The block runs in a transparent scope ([`Scope::child_transparent`])
//! allocated in the **call-site region** — not a per-call frame — so forwarded
//! binds and functions defined in the block stay live after the block ends.
//! A bind colliding with a surfaced member is rejected in
//! [`Scope::bind_value`](crate::machine::core::Scope)'s borrowed-window arm.
//!
//! Only `data` and `functions` are surfaced; `Module::type_members` is not in
//! `Bindings`, so abstract ascriptions stay opaque inside the block.
//!
//! Functor-result escape soundness: the opened module's child scope lives in a
//! per-call `CallFrame` kept alive by an `Rc` on the eager `m`. That `Rc` would
//! drop when `m` drops at body return, so we root a copy of the module value
//! in the call-site region. An escaping closure captures the transparent scope,
//! which anchors the call-site frame, which keeps the rooted `Rc` alive.
//! Top-level modules carry no `Rc` and need no rooting.

use crate::machine::model::types::KKind;
use crate::machine::model::KType;
use crate::machine::{KError, KErrorKind, Scope};

use super::{arg, kw, sig};

/// Mint the transparent child scope and dispatch the body into it as a block (`InScope` — a
/// multi-statement body fans out one sub-dispatch per statement), forwarding the final
/// statement's value as the USING result. The window's surfaced members resolve through
/// [`Scope::binding_cutoff`]'s index-0 (no-cutoff) rule for a borrowed window.
pub fn body<'a>(
    ctx: &crate::machine::core::kfunction::action::BodyCtx<'a, '_>,
) -> crate::machine::core::kfunction::action::Action<'a> {
    use crate::machine::core::kfunction::action::{
        arg_held, require_kexpression, Action, AwaitContinue, Dep, DepPlacement,
    };
    use crate::machine::model::values::Held;

    let module = match arg_held(ctx.args, "m") {
        Some(Held::Type(KType::Module { module: m })) => *m,
        Some(Held::Type(other)) => {
            return Action::Done(Err(KError::new(KErrorKind::TypeMismatch {
                arg: "m".to_string(),
                expected: "Module".to_string(),
                got: other.name(),
            })))
        }
        Some(Held::Object(other)) => {
            return Action::Done(Err(KError::new(KErrorKind::TypeMismatch {
                arg: "m".to_string(),
                expected: "Module".to_string(),
                got: other.ktype().name().to_string(),
            })))
        }
        None => return Action::Done(Err(KError::new(KErrorKind::MissingArg("m".to_string())))),
    };
    let body_expr = crate::try_action!(require_kexpression(ctx.args, "USING", "body"));
    // Root the opened module's reach on the call-site scope. Its child-scope region (a functor
    // result's per-call frame) is pinned only by the eager `m` dep across this step, but the
    // transparent `child` window below borrows that region and outlives the step — and any closure
    // escaping the block captures `child`. Folding the `m` carrier's reach onto `ctx.scope` (where
    // `child` lives) keeps the region alive for the window's life, the carrier-delivered analogue of
    // the old relocate-seam reach reconstruction. A top-level module reaches no per-call region, so
    // `fold_reach`'s home/ancestor omission folds nothing.
    if let Some(carrier) = ctx.arg_carrier("m") {
        ctx.scope.fold_reach(carrier.witness());
    }
    // Transparent scope lives in the call-site region so forwarded binds and block-defined
    // functions outlive the block.
    let module_bindings = module.child_scope().bindings();
    let child: &'a Scope<'a> = ctx
        .scope
        .region
        .alloc_scope(Scope::child_transparent(ctx.scope, module_bindings));
    let finish: AwaitContinue<'a> = Box::new(move |_fctx, results| {
        // The body block's final statement value is the USING result.
        Action::Done(Ok(*results
            .last()
            .expect("USING body yields at least one value")))
    });
    Action::AwaitDeps {
        deps: vec![Dep::Dispatch {
            expr: body_expr,
            placement: DepPlacement::InScope(child),
        }],
        finish,
    }
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    let signature = sig(
        KType::Any,
        vec![
            kw("USING"),
            arg("m", KType::OfKind(KKind::Module)),
            kw("SCOPE"),
            arg("body", KType::KExpression),
        ],
    );
    crate::builtins::register_builtin(scope, "USING", signature, body);
}

#[cfg(test)]
mod tests;
