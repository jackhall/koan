use crate::machine::model::TypeRegistry;
use crate::machine::model::{Carried, KObject, KType};
use crate::machine::WriteGate;
use crate::machine::{KError, KErrorKind, Scope};

use super::{arg, kw, sig};

/// `PRINT <msg:Any>` — renders the `msg` object cell, writes it plus a newline to the run's
/// output sink, and returns the rendered string as a `KObject::KString` value.
pub fn body<'a>(ctx: &crate::machine::BodyCtx<'_, 'a, '_>) -> crate::machine::Action<'a> {
    use crate::machine::{arg_held, Action};
    // `msg` is an `Any` slot, so render whichever arm the carrier holds (object or type) via
    // `Held::summarize`.
    let rendered = match arg_held(ctx.args, "msg") {
        Some(value) => value.summarize(ctx.types),
        None => return Action::done(Err(KError::new(KErrorKind::MissingArg("msg".to_string())))),
    };
    let line = format!("{rendered}\n");
    ctx.out.write_out(line.as_bytes());
    // The rendered bytes are bumped into this step's own destination region through a zero-dep fold,
    // so the carrier reaches nothing but the region it lives in — the active frame is folded in at
    // finalize/close, not bundled here.
    let carrier = ctx.ctx.alloc_carried_with(&[], |brand, _| {
        Carried::Object(
            brand.alloc_object_folded(KObject::KString(brand.allocator().text(&rendered))),
        )
    });
    Action::done(Ok(carrier))
}

pub fn register<'a>(scope: &'a Scope<'a>, types: &TypeRegistry, gate: &mut WriteGate) {
    let signature = sig(KType::STR, vec![kw("PRINT"), arg("msg", KType::ANY)]);
    crate::builtins::register_builtin(scope, "PRINT", signature, body, types, gate);
}
