use crate::machine::model::{KObject, KType};
use crate::machine::{KError, KErrorKind, Scope};

use super::{arg, kw, sig};

/// `PRINT <msg:Any>` — renders the `msg` object cell, writes it plus a newline to
/// `ctx.scope`'s nearest `out`, and returns the rendered string as a `KObject::KString` value.
pub fn body<'a>(ctx: &crate::machine::BodyCtx<'a, '_>) -> crate::machine::Action<'a> {
    use crate::machine::{arg_held, Action};
    // `msg` is an `Any` slot, so render whichever arm the carrier holds (object or type) via
    // `Held::summarize`.
    let rendered = match arg_held(ctx.args, "msg") {
        Some(value) => value.summarize(),
        None => return Action::Done(Err(KError::new(KErrorKind::MissingArg("msg".to_string())))),
    };
    let line = format!("{rendered}\n");
    ctx.scope.write_out(line.as_bytes());
    // The rendered string is owned (region-pure), so it allocs through the witnessed surface born
    // under the empty (foreign-reach-only) set — the active frame is folded in at finalize/close, not
    // bundled here.
    let carrier = ctx
        .scope
        .brand()
        .alloc_object_witnessed(KObject::KString(rendered));
    Action::Done(Ok(carrier))
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    let signature = sig(KType::Str, vec![kw("PRINT"), arg("msg", KType::Any)]);
    crate::builtins::register_builtin(scope, "PRINT", signature, body);
}
