use crate::runtime::machine::model::{KObject, KType, Parseable};
use crate::runtime::machine::{ArgumentBundle, BodyResult, Scope, SchedulerHandle};

use super::{arg, err, kw, register_builtin, sig};

/// `PRINT <msg:Any>` — renders the bound value via `Parseable::summarize`, writes it to the
/// nearest `out` writer (via `Scope::write_out`, which walks the `outer` chain) followed by
/// a newline, and returns the rendered string (without the trailing newline) so the call
/// composes with enclosing expressions.
pub fn body<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let rendered = match bundle.require("msg") {
        Ok(obj) => obj.summarize(),
        Err(e) => return err(e),
    };
    let line = format!("{rendered}\n");
    scope.write_out(line.as_bytes());
    BodyResult::Value(scope.arena.alloc_object(KObject::KString(rendered)))
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    register_builtin(
        scope,
        "PRINT",
        sig(KType::Str, vec![kw("PRINT"), arg("msg", KType::Any)]),
        body,
    );
}
