use std::rc::Rc;

use crate::dispatch::kfunction::{Argument, ArgumentBundle, ExpressionSignature, KType, SignatureElement};
use crate::dispatch::kobject::KObject;
use crate::dispatch::scope::Scope;
use crate::try_args;

use super::{null, register_builtin};

/// `IF <predicate:Bool> THEN <value:KExpression>` — the lazy form. When `predicate` is false,
/// the captured `value` expression is never touched. When true, dispatches the captured
/// expression against `scope` and returns the produced `KObject`. Bare atoms inside parens
/// (e.g. `(99)`, `(some_var)`) dispatch through the `value_lookup`/`value_pass` builtins.
pub fn body<'a>(scope: &mut Scope<'a>, mut bundle: ArgumentBundle<'a>) -> &'a KObject<'a> {
    try_args!(bundle, return null(); predicate: Bool);
    if !predicate {
        return null();
    }
    let value_rc = match bundle.args.remove("value") {
        Some(rc) => rc,
        None => return null(),
    };
    let expr = match Rc::try_unwrap(value_rc) {
        Ok(KObject::KExpression(e)) => e,
        Ok(_) => return null(),
        Err(rc) => match &*rc {
            KObject::KExpression(e) => e.clone(),
            _ => return null(),
        },
    };
    let future = match scope.dispatch(expr) {
        Ok(f) => f,
        Err(_) => return null(),
    };
    let inner = future.function.body;
    inner(scope, future.bundle)
}

pub fn register(scope: &mut Scope<'static>) {
    register_builtin(
        scope,
        "if_then",
        ExpressionSignature {
            return_type: KType::Any,
            elements: vec![
                SignatureElement::Token("IF".into()),
                SignatureElement::Argument(Argument { name: "predicate".into(), ktype: KType::Bool }),
                SignatureElement::Token("THEN".into()),
                SignatureElement::Argument(Argument { name: "value".into(),     ktype: KType::KExpression }),
            ],
        },
        body,
    );
}

#[cfg(test)]
mod tests {
    use crate::dispatch::builtins::default_scope;
    use crate::dispatch::kobject::KObject;
    use crate::parse::kexpression::{ExpressionPart, KExpression, KLiteral};

    #[test]
    fn dispatch_if_then_expression() {
        let mut scope = default_scope();
        // IF true THEN (99) — value side parens-wrapped so it's an Expression that the
        // lazy if_then captures and then dispatches via `value_pass`.
        let inner = KExpression {
            parts: vec![ExpressionPart::Literal(KLiteral::Number(99.0))],
        };
        let expr = KExpression {
            parts: vec![
                ExpressionPart::Token("IF".into()),
                ExpressionPart::Literal(KLiteral::Boolean(true)),
                ExpressionPart::Token("THEN".into()),
                ExpressionPart::Expression(Box::new(inner)),
            ],
        };

        let future = scope.dispatch(expr).expect("dispatch should match `if_then`");
        let body = future.function.body;
        let result = body(&mut scope, future.bundle);

        assert!(matches!(result, KObject::Number(n) if *n == 99.0));
    }

    #[test]
    fn dispatch_lazy_if_then_captures_expression_as_data() {
        let mut scope = default_scope();
        let inner = KExpression {
            parts: vec![
                ExpressionPart::Token("LET".into()),
                ExpressionPart::Token("z".into()),
                ExpressionPart::Token("=".into()),
                ExpressionPart::Literal(KLiteral::Number(11.0)),
            ],
        };
        let expr = KExpression {
            parts: vec![
                ExpressionPart::Token("IF".into()),
                ExpressionPart::Literal(KLiteral::Boolean(true)),
                ExpressionPart::Token("THEN".into()),
                ExpressionPart::Expression(Box::new(inner)),
            ],
        };

        let future = scope.dispatch(expr).expect("dispatch should match lazy if_then");
        // The bundle's `value` arg is captured as a KExpression, not eagerly resolved.
        assert!(matches!(
            future.bundle.get("value"),
            Some(KObject::KExpression(_))
        ));

        let body = future.function.body;
        let result = body(&mut scope, future.bundle);
        // Lazy body dispatched at runtime: LET ran, returned 11, and bound z.
        assert!(matches!(result, KObject::Number(n) if *n == 11.0));
        assert!(matches!(scope.data.get("z"), Some(KObject::Number(n)) if *n == 11.0));
    }

    #[test]
    fn dispatch_lazy_if_then_false_skips_expression() {
        let mut scope = default_scope();
        let inner = KExpression {
            parts: vec![
                ExpressionPart::Token("LET".into()),
                ExpressionPart::Token("skipped".into()),
                ExpressionPart::Token("=".into()),
                ExpressionPart::Literal(KLiteral::Number(1.0)),
            ],
        };
        let expr = KExpression {
            parts: vec![
                ExpressionPart::Token("IF".into()),
                ExpressionPart::Literal(KLiteral::Boolean(false)),
                ExpressionPart::Token("THEN".into()),
                ExpressionPart::Expression(Box::new(inner)),
            ],
        };

        let future = scope.dispatch(expr).expect("dispatch should match lazy if_then");
        let body = future.function.body;
        let result = body(&mut scope, future.bundle);

        assert!(matches!(result, KObject::Null));
        assert!(scope.data.get("skipped").is_none());
    }
}
