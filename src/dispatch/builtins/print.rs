use crate::dispatch::runtime::{KError, KErrorKind};
use crate::dispatch::kfunction::{ArgumentBundle, BodyResult, SchedulerHandle};
use crate::dispatch::types::{Argument, ExpressionSignature, KType, Parseable, SignatureElement};
use crate::dispatch::values::KObject;
use crate::dispatch::runtime::Scope;

use super::{err, register_builtin};

/// `PRINT <msg:Any>` — renders the bound value via `Parseable::summarize`, writes it to the
/// nearest `out` writer (via `Scope::write_out`, which walks the `outer` chain) followed by
/// a newline, and returns the rendered string (without the trailing newline) so the call
/// composes with enclosing expressions.
pub fn body<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let rendered = match bundle.get("msg") {
        Some(obj) => obj.summarize(),
        None => return err(KError::new(KErrorKind::MissingArg("msg".to_string()))),
    };
    let line = format!("{rendered}\n");
    scope.write_out(line.as_bytes());
    BodyResult::Value(scope.arena.alloc_object(KObject::KString(rendered)))
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    register_builtin(
        scope,
        "PRINT",
        ExpressionSignature {
            return_type: KType::Str,
            elements: vec![
                SignatureElement::Keyword("PRINT".into()),
                SignatureElement::Argument(Argument { name: "msg".into(), ktype: KType::Any }),
            ],
        },
        body,
    );
}
