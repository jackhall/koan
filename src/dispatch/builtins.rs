use std::collections::HashMap;
use std::io::Write;

use super::kfunction::{Argument, ArgumentBundle, ExpressionSignature, KFunction, KType, SignatureElement};
use super::kobject::KObject;
use super::scope::Scope;

fn null<'a>() -> &'a KObject<'a> {
    Box::leak(Box::new(KObject::Null))
}

/// `let <name:Identifier> = <value:Any>` — copies the bound value (scalars only) into a
/// `Box::leak`'d `KObject` so it satisfies `Scope::add`'s `&'a KObject<'a>` signature, inserts
/// it under `name`, and returns that same leaked reference. Non-scalar values are silently
/// dropped and produce a freshly leaked `KObject::Null`.
pub fn builtin_let<'a>(scope: &mut Scope<'a>, bundle: ArgumentBundle<'a>) -> &'a KObject<'a> {
    let name = match bundle.get("name") {
        Some(KObject::KString(s)) => s.clone(),
        _ => return null(),
    };
    let cloned = match bundle.get("value") {
        Some(KObject::Number(n)) => KObject::Number(*n),
        Some(KObject::KString(s)) => KObject::KString(s.clone()),
        Some(KObject::Bool(b)) => KObject::Bool(*b),
        Some(KObject::Null) => KObject::Null,
        _ => return null(),
    };
    let leaked: &'a KObject<'a> = Box::leak(Box::new(cloned));
    scope.add(name, leaked);
    leaked
}

/// `print <msg:Str>` — writes the bound `KString` to `scope.out`, followed by a newline.
pub fn builtin_print<'a>(scope: &mut Scope<'a>, bundle: ArgumentBundle<'a>) -> &'a KObject<'a> {
    if let Some(KObject::KString(s)) = bundle.get("msg") {
        let _ = writeln!(scope.out, "{s}");
    }
    null()
}

/// Build a fresh root scope populated with the language's builtin `KFunction`s. Each call
/// `Box::leak`s its own function and object boxes, so the returned scope is `'static` and child
/// scopes can chain off it via `Scope.outer` to inherit the builtins.
pub fn default_scope() -> Scope<'static> {
    let mut scope = Scope {
        outer: None,
        data: HashMap::new(),
        functions: Vec::new(),
        out: Box::new(std::io::stdout()),
    };

    let let_fn: &'static KFunction<'static> = Box::leak(Box::new(KFunction::new(
        None,
        ExpressionSignature {
            return_type: KType::Null,
            elements: vec![
                SignatureElement::Token("let".into()),
                SignatureElement::Argument(Argument { name: "name".into(),  ktype: KType::Identifier, variadic: false }),
                SignatureElement::Token("=".into()),
                SignatureElement::Argument(Argument { name: "value".into(), ktype: KType::Any,        variadic: false }),
            ],
        },
        builtin_let,
    )));
    let let_obj: &'static KObject<'static> = Box::leak(Box::new(KObject::KFunction(let_fn)));
    scope.add("let".into(), let_obj);

    let print_fn: &'static KFunction<'static> = Box::leak(Box::new(KFunction::new(
        None,
        ExpressionSignature {
            return_type: KType::Null,
            elements: vec![
                SignatureElement::Token("print".into()),
                SignatureElement::Argument(Argument { name: "msg".into(), ktype: KType::Str, variadic: false }),
            ],
        },
        builtin_print,
    )));
    let print_obj: &'static KObject<'static> = Box::leak(Box::new(KObject::KFunction(print_fn)));
    scope.add("print".into(), print_obj);

    scope
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::rc::Rc;

    use super::{builtin_let, default_scope, Scope};
    use crate::dispatch::kfunction::ArgumentBundle;
    use crate::dispatch::kobject::KObject;
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

        let result = builtin_let(&mut scope, ArgumentBundle { args });

        assert!(matches!(result, KObject::Number(n) if *n == 42.0));
        let entry = scope.data.get("x").expect("expected binding 'x'");
        assert!(matches!(entry, KObject::Number(n) if *n == 42.0));
    }

    #[test]
    fn dispatch_let_expression() {
        let mut scope = default_scope();
        let expr = KExpression {
            parts: vec![
                ExpressionPart::Token("let".into()),
                ExpressionPart::Token("x".into()),
                ExpressionPart::Token("=".into()),
                ExpressionPart::Literal(KLiteral::Number(42.0)),
            ],
        };

        let future = scope.dispatch(expr).expect("dispatch should match `let`");
        let body = future.function.body;
        let bundle = future.bundle;
        let result = body(&mut scope, bundle);

        assert!(matches!(result, KObject::Number(n) if *n == 42.0));
        let entry = scope.data.get("x").expect("expected binding 'x'");
        assert!(matches!(entry, KObject::Number(n) if *n == 42.0));
    }
}
