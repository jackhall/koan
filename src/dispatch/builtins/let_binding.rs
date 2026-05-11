use crate::dispatch::{
    Argument, ArgumentBundle, BodyResult, ExpressionSignature, KError, KErrorKind, KObject, KType,
    Scope, SchedulerHandle, SignatureElement,
};
use crate::parse::kexpression::{ExpressionPart, KExpression};

use super::{err, register_builtin_with_pre_run};

/// `LET <name> = <value:Any>` — copies the bound value into an arena-allocated `KObject`,
/// inserts it under `name`, and returns that same arena reference. Compound values recurse
/// through `KObject::deep_clone`.
///
/// Two overloads share this body, differing only in the `name` slot's `KType`: `Identifier`
/// (the original lowercase-name path) and `TypeExprRef` (added in module-system stage 1 so
/// `LET ModuleName = (...)` can bind a name that classifies as a Type token per §2).
pub fn body<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let name = match bundle.get("name") {
        Some(KObject::KString(s)) => s.clone(),
        Some(KObject::TypeExprValue(t)) => t.name.clone(),
        Some(other) => {
            return err(KError::new(KErrorKind::TypeMismatch {
                arg: "name".to_string(),
                expected: "Identifier or TypeExprRef".to_string(),
                got: other.ktype().name(),
            }));
        }
        None => return err(KError::new(KErrorKind::MissingArg("name".to_string()))),
    };
    let cloned = match bundle.get("value") {
        Some(obj) => obj.deep_clone(),
        None => return err(KError::new(KErrorKind::MissingArg("value".to_string()))),
    };
    let arena = scope.arena;
    let allocated: &'a KObject<'a> = arena.alloc_object(cloned);
    if let Err(e) = scope.bind_value(name, allocated) {
        return err(e);
    }
    BodyResult::Value(allocated)
}

/// Dispatch-time placeholder extractor for LET. Both overloads (`LET <name:Identifier> = ...`
/// and `LET <name:TypeExprRef> = ...`) put the bound name at `parts[1]`; pull it out
/// structurally without dispatching anything. Returns `None` on shape mismatch (the body
/// will surface a structured error later).
pub(crate) fn pre_run(expr: &KExpression<'_>) -> Option<String> {
    match expr.parts.get(1)? {
        ExpressionPart::Identifier(s) => Some(s.clone()),
        ExpressionPart::Type(t) => Some(t.name.clone()),
        _ => None,
    }
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    register_builtin_with_pre_run(
        scope,
        "LET",
        ExpressionSignature {
            return_type: KType::Any,
            elements: vec![
                SignatureElement::Keyword("LET".into()),
                SignatureElement::Argument(Argument { name: "name".into(),  ktype: KType::Identifier }),
                SignatureElement::Keyword("=".into()),
                SignatureElement::Argument(Argument { name: "value".into(), ktype: KType::Any }),
            ],
        },
        body,
        Some(pre_run),
    );
    register_builtin_with_pre_run(
        scope,
        "LET",
        ExpressionSignature {
            return_type: KType::Any,
            elements: vec![
                SignatureElement::Keyword("LET".into()),
                SignatureElement::Argument(Argument { name: "name".into(),  ktype: KType::TypeExprRef }),
                SignatureElement::Keyword("=".into()),
                SignatureElement::Argument(Argument { name: "value".into(), ktype: KType::Any }),
            ],
        },
        body,
        Some(pre_run),
    );
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::rc::Rc;

    use super::body;
    use crate::dispatch::builtins::default_scope;
    use crate::dispatch::builtins::test_support::run_root_bare;
    use crate::dispatch::{ArgumentBundle, BodyResult, KObject};
    use crate::execute::scheduler::Scheduler;
    use crate::parse::kexpression::{ExpressionPart, KExpression, KLiteral};

    #[test]
    fn let_inserts_binding_into_scope() {
        use crate::dispatch::RuntimeArena;
        let arena = RuntimeArena::new();
        let scope = run_root_bare(&arena);
        let mut sched = Scheduler::new();
        let mut args = HashMap::new();
        args.insert("name".to_string(), Rc::new(KObject::KString("x".into())));
        args.insert("value".to_string(), Rc::new(KObject::Number(42.0)));

        let result = body(scope, &mut sched, ArgumentBundle { args });

        let value = match result {
            BodyResult::Value(v) => v,
            BodyResult::Tail { .. } => panic!("LET should not produce a Tail"),
            BodyResult::DeferTo(_) => panic!("LET should not produce a DeferTo"),
            BodyResult::Err(e) => panic!("LET errored unexpectedly: {e}"),
        };
        assert!(matches!(value, KObject::Number(n) if *n == 42.0));
        let data = scope.data.borrow();
        let entry = data.get("x").expect("expected binding 'x'");
        assert!(matches!(entry, KObject::Number(n) if *n == 42.0));
    }

    /// Smoke test for LET's pre_run extractor: structural extraction of `parts[1]`
    /// returns the bound name without requiring sub-dispatches.
    #[test]
    fn pre_run_extracts_let_name() {
        use crate::parse::expression_tree::parse;
        let mut exprs = parse("LET hello = 1").expect("parse should succeed");
        let expr = exprs.remove(0);
        let name = super::pre_run(&expr);
        assert_eq!(name.as_deref(), Some("hello"));
    }

    /// End-to-end install-then-clear: dispatch `LET x = 1` through the scheduler. The
    /// pre_run hook installs `placeholders["x"] = NodeId(...)` before the body runs;
    /// after the body finalizes via `bind_value`, the placeholder is removed.
    #[test]
    fn pre_run_install_then_body_finalize_clears_placeholder() {
        use crate::dispatch::RuntimeArena;
        use crate::execute::scheduler::Scheduler;
        use crate::dispatch::builtins::default_scope;
        use crate::parse::expression_tree::parse;
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let mut sched = Scheduler::new();
        let exprs = parse("LET hello = 1").unwrap();
        for e in exprs { sched.add_dispatch(e, scope); }
        sched.execute().unwrap();
        // After execute, placeholders should not contain "hello" — bind_value cleared it.
        assert!(scope.placeholders.borrow().get("hello").is_none());
        assert!(matches!(scope.lookup("hello"), Some(KObject::Number(n)) if *n == 1.0));
    }

    #[test]
    fn dispatch_let_expression() {
        use crate::dispatch::RuntimeArena;
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let expr = KExpression {
            parts: vec![
                ExpressionPart::Keyword("LET".into()),
                ExpressionPart::Identifier("x".into()),
                ExpressionPart::Keyword("=".into()),
                ExpressionPart::Literal(KLiteral::Number(42.0)),
            ],
        };

        let mut sched = Scheduler::new();
        let id = sched.add_dispatch(expr, scope);
        sched.execute().unwrap();

        assert!(matches!(sched.read(id), KObject::Number(n) if *n == 42.0));
        let data = scope.data.borrow();
        let entry = data.get("x").expect("expected binding 'x'");
        assert!(matches!(entry, KObject::Number(n) if *n == 42.0));
    }
}
