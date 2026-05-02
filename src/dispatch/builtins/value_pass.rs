use crate::dispatch::kfunction::{
    Argument, ArgumentBundle, BodyResult, ExpressionSignature, KType, SchedulerHandle,
    SignatureElement,
};
use crate::dispatch::scope::Scope;

use super::{null, register_builtin};

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
        None => return null(),
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
    use crate::dispatch::kfunction::{ArgumentBundle, BodyResult};
    use crate::dispatch::kobject::KObject;
    use crate::dispatch::scope::Scope;
    use crate::execute::scheduler::Scheduler;

    #[test]
    fn value_pass_returns_literal() {
        use crate::dispatch::arena::RuntimeArena;
        let arena = RuntimeArena::new();
        let scope = arena.alloc_scope(Scope::run_root(&arena, None, Box::new(std::io::sink())));
        let mut sched = Scheduler::new();
        let mut args = HashMap::new();
        args.insert("v".to_string(), Rc::new(KObject::Number(7.0)));

        let result = match body(scope, &mut sched, ArgumentBundle { args }) {
            BodyResult::Value(v) => v,
            BodyResult::Tail { .. } => panic!("value_pass should not produce a Tail"),
        };

        assert!(matches!(result, KObject::Number(n) if *n == 7.0));
    }
}
