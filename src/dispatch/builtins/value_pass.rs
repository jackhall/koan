use crate::dispatch::runtime::{KError, KErrorKind};
use crate::dispatch::kfunction::{ArgumentBundle, BodyResult, SchedulerHandle};
use crate::dispatch::types::{Argument, ExpressionSignature, KType, SignatureElement};
use crate::dispatch::runtime::Scope;

use super::{err, register_builtin};

/// `<v:Any>` — single-part expression containing a literal (or a previously-evaluated future).
/// Returns the value as a fresh arena-allocated `KObject` via `deep_clone`. Combined with
/// `value_lookup` this lets parens-wrapped atoms — `(99)`, `("x")`, `(some_var)`, `([1 2 3])`
/// — dispatch through the regular pipeline.
pub fn body<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let cloned = match bundle.get("v") {
        Some(obj) => obj.deep_clone(),
        None => return err(KError::new(KErrorKind::MissingArg("v".to_string()))),
    };
    let arena = scope.arena;
    BodyResult::Value(arena.alloc_object(cloned))
}

pub fn register<'a>(scope: &'a Scope<'a>) {
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
    use crate::dispatch::builtins::test_support::run_root_bare;
    use crate::dispatch::kfunction::{ArgumentBundle, BodyResult};
    use crate::dispatch::values::KObject;
    use crate::execute::scheduler::Scheduler;

    #[test]
    fn value_pass_returns_literal() {
        use crate::dispatch::runtime::RuntimeArena;
        let arena = RuntimeArena::new();
        let scope = run_root_bare(&arena);
        let mut sched = Scheduler::new();
        let mut args = HashMap::new();
        args.insert("v".to_string(), Rc::new(KObject::Number(7.0)));

        let result = match body(scope, &mut sched, ArgumentBundle { args }) {
            BodyResult::Value(v) => v,
            BodyResult::Tail { .. } => panic!("value_pass should not produce a Tail"),
            BodyResult::DeferTo(_) => panic!("value_pass should not produce a DeferTo"),
            BodyResult::Err(e) => panic!("value_pass errored unexpectedly: {e}"),
        };

        assert!(matches!(result, KObject::Number(n) if *n == 7.0));
    }
}
