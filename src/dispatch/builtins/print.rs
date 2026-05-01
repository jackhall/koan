use std::io::Write;

use crate::dispatch::kfunction::{Argument, ArgumentBundle, ExpressionSignature, KType, SignatureElement};
use crate::dispatch::kobject::KObject;
use crate::dispatch::scope::Scope;

use super::{null, register_builtin};

/// `PRINT <msg:Str>` — writes the bound `KString` to `scope.out`, followed by a newline.
pub fn body<'a>(scope: &mut Scope<'a>, bundle: ArgumentBundle<'a>) -> &'a KObject<'a> {
    if let Some(KObject::KString(s)) = bundle.get("msg") {
        let _ = writeln!(scope.out, "{s}");
    }
    null()
}

pub fn register(scope: &mut Scope<'static>) {
    register_builtin(
        scope,
        "PRINT",
        ExpressionSignature {
            return_type: KType::Null,
            elements: vec![
                SignatureElement::Token("PRINT".into()),
                SignatureElement::Argument(Argument { name: "msg".into(), ktype: KType::Str }),
            ],
        },
        body,
    );
}
