//! `USING <module> SCOPE <block>` — block-scoped module opening. See
//! `design/typing/modules.md` § "Block-scoped opening".
//!
//! `m` is eager (a resolved module value), `body` is lazy
//! ([`KType::KExpression`]) so it evaluates in the opened scope.
//!
//! The block runs in a transparent scope ([`Scope::child_transparent`])
//! allocated in the **call-site arena** — not a per-call frame — so forwarded
//! binds and functions defined in the block stay live after the block ends.
//! A bind colliding with a surfaced member is rejected in
//! [`Scope::bind_value`](crate::machine::core::Scope)'s borrowed-window arm.
//!
//! Only `data` and `functions` are surfaced; `Module::type_members` is not in
//! `Bindings`, so abstract ascriptions stay opaque inside the block.
//!
//! Functor-result escape soundness: the opened module's child scope lives in a
//! per-call `CallArena` kept alive by an `Rc` on the eager `m`. That `Rc` would
//! drop when `m` drops at body return, so we root a copy of the module value
//! in the call-site arena. An escaping closure captures the transparent scope,
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

    let (module, module_frame) = match arg_held(ctx.args, "m") {
        Some(Held::Type(KType::Module {
            module: m,
            frame: anchor,
        })) => (*m, anchor.clone()),
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
    // Root the frame `Rc` in the call-site arena so the borrowed window outlives the eager `m`
    // arg and any escaping closure. No-op for top-level modules.
    if module_frame.is_some() {
        ctx.scope.arena.alloc_ktype(KType::Module {
            module,
            frame: module_frame,
        });
    }
    // Transparent scope lives in the call-site arena so forwarded binds and block-defined
    // functions outlive the block.
    let module_bindings = module.child_scope().bindings();
    let child: &'a Scope<'a> = ctx
        .scope
        .arena
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
