use std::rc::Rc;

use crate::dispatch::kfunction::{
    Argument, ArgumentBundle, BodyResult, ExpressionSignature, KType, SchedulerHandle,
    SignatureElement,
};
use crate::dispatch::kobject::KObject;
use crate::dispatch::scope::Scope;
use crate::try_args;

use super::{null, register_builtin};

/// `IF <predicate:Bool> THEN <value:KExpression>` — the lazy form. When `predicate` is false,
/// the captured `value` expression is never touched. When true, returns the captured expression
/// as a `Tail`: the scheduler rewrites the if_then's own slot to a fresh `Dispatch(value)` and
/// re-runs in place, walking the value's AST, evaluating sub-expressions, and producing its
/// result — all in the slot the if_then originally occupied. Scope is unchanged (`tail` not
/// `tail_with_scope`); the lazy expression evaluates in the same scope as the IF call site.
pub fn body<'a>(
    _scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    mut bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
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
    BodyResult::tail(expr)
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    register_builtin(
        scope,
        "if_then",
        ExpressionSignature {
            return_type: KType::Any,
            elements: vec![
                SignatureElement::Keyword("IF".into()),
                SignatureElement::Argument(Argument { name: "predicate".into(), ktype: KType::Bool }),
                SignatureElement::Keyword("THEN".into()),
                SignatureElement::Argument(Argument { name: "value".into(),     ktype: KType::KExpression }),
            ],
        },
        body,
    );
}

#[cfg(test)]
mod tests {
    use crate::dispatch::arena::RuntimeArena;
    use crate::dispatch::builtins::default_scope;
    use crate::dispatch::kobject::KObject;
    use crate::dispatch::scope::Scope;
    use crate::execute::scheduler::Scheduler;
    use crate::parse::kexpression::{ExpressionPart, KExpression, KLiteral};

    fn run_one<'a>(
        scope: &'a Scope<'a>,
        expr: KExpression<'a>,
    ) -> &'a KObject<'a> {
        let mut sched = Scheduler::new();
        let id = sched.add_dispatch(expr, scope);
        let results = sched.execute().expect("scheduler should succeed");
        results[id.index()]
    }

    #[test]
    fn dispatch_if_then_expression() {
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        // IF true THEN (99) — value side parens-wrapped so it's an Expression that the
        // lazy if_then captures and the scheduler then dispatches via `value_pass`.
        let inner = KExpression {
            parts: vec![ExpressionPart::Literal(KLiteral::Number(99.0))],
        };
        let expr = KExpression {
            parts: vec![
                ExpressionPart::Keyword("IF".into()),
                ExpressionPart::Literal(KLiteral::Boolean(true)),
                ExpressionPart::Keyword("THEN".into()),
                ExpressionPart::Expression(Box::new(inner)),
            ],
        };

        let result = run_one(scope, expr);
        assert!(matches!(result, KObject::Number(n) if *n == 99.0));
    }

    #[test]
    fn dispatch_lazy_if_then_captures_expression_as_data() {
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let inner = KExpression {
            parts: vec![
                ExpressionPart::Keyword("LET".into()),
                ExpressionPart::Identifier("z".into()),
                ExpressionPart::Keyword("=".into()),
                ExpressionPart::Literal(KLiteral::Number(11.0)),
            ],
        };
        let expr = KExpression {
            parts: vec![
                ExpressionPart::Keyword("IF".into()),
                ExpressionPart::Literal(KLiteral::Boolean(true)),
                ExpressionPart::Keyword("THEN".into()),
                ExpressionPart::Expression(Box::new(inner)),
            ],
        };

        let result = run_one(scope, expr);
        // Lazy body deferred to scheduler: LET ran inside the spawned Dispatch, returned 11,
        // and bound z; the IF's result forwards through the spawned node.
        assert!(matches!(result, KObject::Number(n) if *n == 11.0));
        let data = scope.data.borrow();
        assert!(matches!(data.get("z"), Some(KObject::Number(n)) if *n == 11.0));
    }

    #[test]
    fn dispatch_lazy_if_then_false_skips_expression() {
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let inner = KExpression {
            parts: vec![
                ExpressionPart::Keyword("LET".into()),
                ExpressionPart::Identifier("skipped".into()),
                ExpressionPart::Keyword("=".into()),
                ExpressionPart::Literal(KLiteral::Number(1.0)),
            ],
        };
        let expr = KExpression {
            parts: vec![
                ExpressionPart::Keyword("IF".into()),
                ExpressionPart::Literal(KLiteral::Boolean(false)),
                ExpressionPart::Keyword("THEN".into()),
                ExpressionPart::Expression(Box::new(inner)),
            ],
        };

        let result = run_one(scope, expr);
        assert!(matches!(result, KObject::Null));
        let data = scope.data.borrow();
        assert!(data.get("skipped").is_none());
    }
}
