use crate::machine::model::{KObject, KType};
use crate::machine::{KError, KErrorKind, Scope};

use super::{arg, kw, sig};

/// `PRINT <msg:Any>` — renders the `msg` object cell, writes it plus a newline to
/// `ctx.scope`'s nearest `out`, and returns the rendered string as a `KObject::KString` value.
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
    crate::builtins::register_action_builtin(scope, "PRINT", signature, body_action);
}
