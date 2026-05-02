use crate::dispatch::kfunction::{
    Argument, ArgumentBundle, BodyResult, ExpressionSignature, KType, SchedulerHandle,
    SignatureElement,
};
use crate::dispatch::kobject::KObject;
use crate::dispatch::scope::Scope;

use super::{null, register_builtin};

/// `PRINT <msg:Str>` — writes the bound `KString` to the nearest `out` writer (via
/// `Scope::write_out`, which walks the `outer` chain) followed by a newline.
pub fn body<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    if let Some(KObject::KString(s)) = bundle.get("msg") {
        let line = format!("{s}\n");
        scope.write_out(line.as_bytes());
    }
    null()
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    register_builtin(
        scope,
        "PRINT",
        ExpressionSignature {
            return_type: KType::Null,
            elements: vec![
                SignatureElement::Keyword("PRINT".into()),
                SignatureElement::Argument(Argument { name: "msg".into(), ktype: KType::Str }),
            ],
        },
        body,
    );
}
