use crate::dispatch::kfunction::{Argument, ArgumentBundle, ExpressionSignature, KType, SignatureElement};
use crate::dispatch::kobject::KObject;
use crate::dispatch::scope::Scope;

use super::{clone_scalar, null, register_builtin};

/// `<v:Any>` — single-part expression containing a literal (or a previously-evaluated future).
/// Returns the value as a fresh leaked `KObject`. Combined with `builtin_value_lookup`
/// (registered first) this lets parens-wrapped atoms — `(99)`, `("x")`, `(some_var)` —
/// dispatch through the regular pipeline.
pub fn body<'a>(_scope: &mut Scope<'a>, bundle: ArgumentBundle<'a>) -> &'a KObject<'a> {
    let cloned = match bundle.get("v").and_then(clone_scalar) {
        Some(v) => v,
        None => return null(),
    };
    Box::leak(Box::new(cloned))
}

pub fn register(scope: &mut Scope<'static>) {
    register_builtin(
        scope,
        "value_pass",
        ExpressionSignature {
            return_type: KType::Any,
            elements: vec![
                SignatureElement::Argument(Argument { name: "v".into(), ktype: KType::Any, variadic: false }),
            ],
        },
        body,
    );
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::rc::Rc;

    use super::body;
    use crate::dispatch::kfunction::ArgumentBundle;
    use crate::dispatch::kobject::KObject;
    use crate::dispatch::scope::Scope;

    #[test]
    fn value_pass_returns_literal() {
        let mut scope = Scope {
            outer: None,
            data: HashMap::new(),
            functions: Vec::new(),
            out: Box::new(std::io::sink()),
        };
        let mut args = HashMap::new();
        args.insert("v".to_string(), Rc::new(KObject::Number(7.0)));

        let result = body(&mut scope, ArgumentBundle { args });

        assert!(matches!(result, KObject::Number(n) if *n == 7.0));
    }
}
