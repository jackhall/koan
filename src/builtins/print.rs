use crate::machine::model::{KObject, KType};
use crate::machine::{ArgumentBundle, BodyResult, KError, KErrorKind, SchedulerHandle, Scope};

use super::{arg, err, kw, register_builtin, sig};

/// `PRINT <msg:Any>` — renders the bound argument's surface form, writes it to the nearest
/// `out` writer (via `Scope::write_out`, which walks the `outer` chain) followed by a
/// newline, and returns the rendered string (without the trailing newline) so the call
/// composes with enclosing expressions. The `Any` slot admits both runtime values and
/// first-class types, so it renders either arm of the carrier.
pub fn body<'a, 's>(
    scope: &'s Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a, 's>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let rendered = match bundle.args.get("msg") {
        Some(value) => value.summarize(),
        None => return err(KError::new(KErrorKind::MissingArg("msg".to_string()))),
    };
    let line = format!("{rendered}\n");
    scope.write_out(line.as_bytes());
    BodyResult::value(scope.arena.alloc_object(KObject::KString(rendered)))
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    register_builtin(
        scope,
        "PRINT",
        sig(KType::Str, vec![kw("PRINT"), arg("msg", KType::Any)]),
        body,
    );
}
