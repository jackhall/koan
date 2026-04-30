use crate::dispatch::kfunction::{Argument, ArgumentBundle, ExpressionSignature, KType, SignatureElement};
use crate::dispatch::kobject::KObject;
use crate::dispatch::scope::Scope;
use crate::try_args;

use super::{clone_scalar, null, register_builtin};

/// `LET <name:Identifier> = <value:Any>` — copies the bound value (scalars only) into a
/// `Box::leak`'d `KObject` so it satisfies `Scope::add`'s `&'a KObject<'a>` signature, inserts
/// it under `name`, and returns that same leaked reference. Non-scalar values are silently
/// dropped and produce a freshly leaked `KObject::Null`.
pub fn body<'a>(scope: &mut Scope<'a>, bundle: ArgumentBundle<'a>) -> &'a KObject<'a> {
    try_args!(bundle, return null(); name: KString);
    let cloned = match bundle.get("value").and_then(clone_scalar) {
        Some(v) => v,
        None => return null(),
    };
    let leaked: &'a KObject<'a> = Box::leak(Box::new(cloned));
    scope.add(name, leaked);
    leaked
}

pub fn register(scope: &mut Scope<'static>) {
    register_builtin(
        scope,
        "LET",
        ExpressionSignature {
            return_type: KType::Null,
            elements: vec![
                SignatureElement::Token("LET".into()),
                SignatureElement::Argument(Argument { name: "name".into(),  ktype: KType::Identifier, variadic: false }),
                SignatureElement::Token("=".into()),
                SignatureElement::Argument(Argument { name: "value".into(), ktype: KType::Any,        variadic: false }),
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
    use crate::dispatch::builtins::default_scope;
    use crate::dispatch::kfunction::ArgumentBundle;
    use crate::dispatch::kobject::KObject;
    use crate::dispatch::scope::Scope;
    use crate::parse::kexpression::{ExpressionPart, KExpression, KLiteral};

    #[test]
    fn let_inserts_binding_into_scope() {
        let mut scope = Scope {
            outer: None,
            data: HashMap::new(),
            functions: Vec::new(),
            out: Box::new(std::io::sink()),
        };
        let mut args = HashMap::new();
        args.insert("name".to_string(), Rc::new(KObject::KString("x".into())));
        args.insert("value".to_string(), Rc::new(KObject::Number(42.0)));

        let result = body(&mut scope, ArgumentBundle { args });

        assert!(matches!(result, KObject::Number(n) if *n == 42.0));
        let entry = scope.data.get("x").expect("expected binding 'x'");
        assert!(matches!(entry, KObject::Number(n) if *n == 42.0));
    }

    #[test]
    fn dispatch_let_expression() {
        let mut scope = default_scope();
        let expr = KExpression {
            parts: vec![
                ExpressionPart::Token("LET".into()),
                ExpressionPart::Token("x".into()),
                ExpressionPart::Token("=".into()),
                ExpressionPart::Literal(KLiteral::Number(42.0)),
            ],
        };

        let future = scope.dispatch(expr).expect("dispatch should match `LET`");
        let body = future.function.body;
        let bundle = future.bundle;
        let result = body(&mut scope, bundle);

        assert!(matches!(result, KObject::Number(n) if *n == 42.0));
        let entry = scope.data.get("x").expect("expected binding 'x'");
        assert!(matches!(entry, KObject::Number(n) if *n == 42.0));
    }
}
