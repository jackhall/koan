use crate::dispatch::kfunction::{Argument, ArgumentBundle, ExpressionSignature, KType, SignatureElement};
use crate::dispatch::kobject::KObject;
use crate::dispatch::scope::Scope;

use super::{null, register_builtin};

/// `<v:Any>` — single-part expression containing a literal (or a previously-evaluated future).
/// Returns the value as a fresh leaked `KObject` via `deep_clone`. Combined with
/// `value_lookup` this lets parens-wrapped atoms — `(99)`, `("x")`, `(some_var)`, `([1 2 3])`
/// — dispatch through the regular pipeline.
pub fn body<'a>(_scope: &mut Scope<'a>, bundle: ArgumentBundle<'a>) -> &'a KObject<'a> {
    let cloned = match bundle.get("v") {
        Some(obj) => obj.deep_clone(),
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
                SignatureElement::Argument(Argument { name: "v".into(), ktype: KType::Any }),
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
        let mut scope = Scope::test_sink();
        let mut args = HashMap::new();
        args.insert("v".to_string(), Rc::new(KObject::Number(7.0)));

        let result = body(&mut scope, ArgumentBundle { args });

        assert!(matches!(result, KObject::Number(n) if *n == 7.0));
    }
}
