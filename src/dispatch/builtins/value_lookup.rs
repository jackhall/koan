use crate::dispatch::kfunction::{Argument, ArgumentBundle, ExpressionSignature, KType, SignatureElement};
use crate::dispatch::kobject::KObject;
use crate::dispatch::scope::Scope;
use crate::try_args;

use super::{null, register_builtin};

/// `<v:Identifier>` — single-part expression containing one name token. Looks `v` up via
/// `Scope::lookup` (which walks the `outer` chain) and returns the bound `KObject`, or `Null`
/// if unbound at every level. Lets a parens-wrapped name like `(some_var)` dispatch and
/// resolve to its current value.
pub fn body<'a>(scope: &mut Scope<'a>, bundle: ArgumentBundle<'a>) -> &'a KObject<'a> {
    try_args!(bundle, return null(); v: KString);
    scope.lookup(&v).unwrap_or_else(null)
}

pub fn register(scope: &mut Scope<'static>) {
    register_builtin(
        scope,
        "value_lookup",
        ExpressionSignature {
            return_type: KType::Any,
            elements: vec![
                SignatureElement::Argument(Argument { name: "v".into(), ktype: KType::Identifier, variadic: false }),
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
    fn value_lookup_returns_binding() {
        let bound: &'static KObject<'static> = Box::leak(Box::new(KObject::Number(42.0)));
        let mut scope = Scope::test_sink();
        scope.data.insert("foo".to_string(), bound);

        let mut args = HashMap::new();
        args.insert("v".to_string(), Rc::new(KObject::KString("foo".into())));

        let result = body(&mut scope, ArgumentBundle { args });

        assert!(matches!(result, KObject::Number(n) if *n == 42.0));
    }

    #[test]
    fn value_lookup_unbound_returns_null() {
        let mut scope = Scope::test_sink();
        let mut args = HashMap::new();
        args.insert("v".to_string(), Rc::new(KObject::KString("missing".into())));

        let result = body(&mut scope, ArgumentBundle { args });

        assert!(matches!(result, KObject::Null));
    }

    #[test]
    fn value_lookup_walks_outer_scope() {
        let bound: &'static KObject<'static> = Box::leak(Box::new(KObject::Number(7.0)));
        let mut outer = Scope::test_sink();
        outer.data.insert("from_outer".to_string(), bound);

        let mut inner = Scope::test_sink();
        inner.outer = Some(&outer);

        let mut args = HashMap::new();
        args.insert("v".to_string(), Rc::new(KObject::KString("from_outer".into())));

        let result = body(&mut inner, ArgumentBundle { args });

        assert!(matches!(result, KObject::Number(n) if *n == 7.0));
    }
}
