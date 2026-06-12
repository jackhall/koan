use crate::machine::model::{KObject, KType};
use crate::machine::{ArgumentBundle, BodyResult, KError, KErrorKind, SchedulerHandle, Scope};

use super::{arg, err, kw, sig};
#[cfg(not(feature = "action-harness"))]
use super::register_builtin;

/// `PRINT <msg:Any>` — renders the bound argument's surface form, writes it to the nearest
/// `out` writer (via `Scope::write_out`, which walks the `outer` chain) followed by a
/// newline, and returns the rendered string (without the trailing newline) so the call
/// composes with enclosing expressions. The `Any` slot admits both runtime values and
/// first-class types, so it renders either arm of the carrier.
pub fn body<'a, 's>(
    sched: &mut dyn SchedulerHandle<'a, 's>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let rendered = match bundle.args.get("msg") {
        Some(value) => value.summarize(),
        None => return err(KError::new(KErrorKind::MissingArg("msg".to_string()))),
    };
    let line = format!("{rendered}\n");
    sched.current_scope().write_out(line.as_bytes());
    BodyResult::value(
        sched
            .current_scope()
            .arena
            .alloc_object(KObject::KString(rendered)),
    )
}

/// `Action`-harness twin of [`body`]: renders the `msg` object cell, writes it plus a newline to
/// `ctx.scope`'s nearest `out`, and returns the rendered string as a `KObject::KString` value.
#[cfg(feature = "action-harness")]
pub fn body_action<'a>(
    ctx: &crate::machine::core::kfunction::action::BodyCtx<'a, '_>,
) -> crate::machine::core::kfunction::action::Action<'a> {
    use crate::machine::core::kfunction::action::{arg_held, Action};
    use crate::machine::model::Carried;
    // `msg` is an `Any` slot, so render whichever arm the carrier holds (object or type) —
    // `Held::summarize` is the twin of the legacy `ArgValue::summarize`.
    let rendered = match arg_held(ctx.args, "msg") {
        Some(value) => value.summarize(),
        None => return Action::Done(Err(KError::new(KErrorKind::MissingArg("msg".to_string())))),
    };
    let line = format!("{rendered}\n");
    ctx.scope.write_out(line.as_bytes());
    let obj = ctx.scope.arena.alloc_object(KObject::KString(rendered));
    Action::Done(Ok(Carried::Object(obj)))
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    let signature = sig(KType::Str, vec![kw("PRINT"), arg("msg", KType::Any)]);
    #[cfg(feature = "action-harness")]
    crate::builtins::register_action_builtin(scope, "PRINT", signature, body_action);
    #[cfg(not(feature = "action-harness"))]
    register_builtin(scope, "PRINT", signature, body);
}
