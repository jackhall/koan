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

use crate::machine::model::{KObject, KType};
use crate::machine::{ArgumentBundle, BodyResult, KError, KErrorKind, Scope, SchedulerHandle};

use crate::machine::core::kfunction::argument_bundle::extract_kexpression;
use super::{arg, err, kw, register_builtin, sig};

pub fn body<'a>(
    scope: &'a Scope<'a>,
    sched: &mut dyn SchedulerHandle<'a>,
    mut bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let (module, module_frame) = match bundle.get("m") {
        Some(KObject::KTypeValue(KType::Module { module: m, frame: anchor })) => {
            (*m, anchor.clone())
        }
        Some(other) => {
            return err(KError::new(KErrorKind::TypeMismatch {
                arg: "m".to_string(),
                expected: "Module".to_string(),
                got: other.ktype().name().to_string(),
            }));
        }
        None => return err(KError::new(KErrorKind::MissingArg("m".to_string()))),
    };
    let body_expr = match extract_kexpression(&mut bundle, "body") {
        Some(e) => e,
        None => {
            return err(KError::new(KErrorKind::ShapeError(
                "USING body slot must be a parenthesized expression".to_string(),
            )));
        }
    };

    // Root the frame `Rc` in the call-site arena so the borrowed window outlives
    // the eager `m` arg and any escaping closure. No-op for top-level modules.
    if module_frame.is_some() {
        scope.arena.alloc(KObject::KTypeValue(KType::Module {
            module,
            frame: module_frame,
        }));
    }

    // Transparent scope lives in the call-site arena so forwarded binds and
    // block-defined functions outlive the block.
    let module_bindings = module.child_scope().bindings();
    let child: &'a Scope<'a> =
        scope.arena.alloc_scope(Scope::child_transparent(scope, module_bindings));
    let sub_id = sched.add_dispatch(body_expr, child);
    BodyResult::DeferTo(sub_id)
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    register_builtin(
        scope,
        "USING",
        sig(KType::Any, vec![
            kw("USING"),
            arg("m", KType::AnyModule),
            kw("SCOPE"),
            arg("body", KType::KExpression),
        ]),
        body,
    );
}

#[cfg(test)]
mod tests;
