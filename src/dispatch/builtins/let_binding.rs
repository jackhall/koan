use crate::dispatch::runtime::{KError, KErrorKind};
use crate::dispatch::kfunction::{ArgumentBundle, BodyResult, SchedulerHandle};
use crate::dispatch::types::{Argument, ExpressionSignature, KType, SignatureElement};
use crate::dispatch::values::KObject;
use crate::dispatch::runtime::Scope;
use crate::try_args;

use super::{err, register_builtin};

/// `LET <name:Identifier> = <value:Any>` — copies the bound value into an arena-allocated
/// `KObject`, inserts it under `name`, and returns that same arena reference. Compound values
/// recurse through `KObject::deep_clone`.
pub fn body<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    try_args!(bundle; name: KString);
    let cloned = match bundle.get("value") {
        Some(obj) => obj.deep_clone(),
        None => return err(KError::new(KErrorKind::MissingArg("value".to_string()))),
    };
    let arena = scope.arena;
    let allocated: &'a KObject<'a> = arena.alloc_object(cloned);
    scope.add(name, allocated);
    BodyResult::Value(allocated)
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    register_builtin(
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
    );
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::rc::Rc;

    use super::body;
    use crate::dispatch::builtins::default_scope;
    use crate::dispatch::kfunction::{ArgumentBundle, BodyResult};
    use crate::dispatch::values::KObject;
    use crate::dispatch::runtime::Scope;
    use crate::execute::scheduler::Scheduler;
    use crate::parse::kexpression::{ExpressionPart, KExpression, KLiteral};

    #[test]
    fn let_inserts_binding_into_scope() {
        use crate::dispatch::runtime::RuntimeArena;
        let arena = RuntimeArena::new();
        let scope = arena.alloc_scope(Scope::run_root(&arena, None, Box::new(std::io::sink())));
        let mut sched = Scheduler::new();
        let mut args = HashMap::new();
        args.insert("name".to_string(), Rc::new(KObject::KString("x".into())));
        args.insert("value".to_string(), Rc::new(KObject::Number(42.0)));

        let result = body(scope, &mut sched, ArgumentBundle { args });

        let value = match result {
            BodyResult::Value(v) => v,
            BodyResult::Tail { .. } => panic!("LET should not produce a Tail"),
            BodyResult::Err(e) => panic!("LET errored unexpectedly: {e}"),
        };
        assert!(matches!(value, KObject::Number(n) if *n == 42.0));
        let data = scope.data.borrow();
        let entry = data.get("x").expect("expected binding 'x'");
        assert!(matches!(entry, KObject::Number(n) if *n == 42.0));
    }

    #[test]
    fn dispatch_let_expression() {
        use crate::dispatch::runtime::RuntimeArena;
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
