use crate::machine::model::KType;
use crate::machine::{ArgumentBundle, BodyResult, Scope, SchedulerHandle};

use super::{arg, err, register_builtin, sig};

/// `<v:Any>` — single-part expression containing a literal (or a previously-evaluated future).
/// Returns the value as a fresh arena-allocated `KObject` via `deep_clone`. Combined with
/// `value_lookup` this lets parens-wrapped atoms — `(99)`, `("x")`, `(some_var)`, `([1 2 3])`
/// — dispatch through the regular pipeline.
pub fn body<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let cloned = match bundle.require("v") {
        Ok(obj) => obj.deep_clone(),
        Err(e) => return err(e),
    };
    let arena = scope.arena;
    BodyResult::Value(arena.alloc_object(cloned))
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    register_builtin(
        scope,
        "value_pass",
        sig(KType::Any, vec![arg("v", KType::Any)]),
        body,
    );
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::rc::Rc;

    use super::body;
    use crate::builtins::test_support::run_root_bare;
    use crate::machine::model::KObject;
    use crate::machine::ArgumentBundle;
    use crate::machine::execute::Scheduler;

    #[test]
    fn value_pass_returns_literal() {
        use crate::machine::RuntimeArena;
        let arena = RuntimeArena::new();
        let scope = run_root_bare(&arena);
        let mut sched = Scheduler::new();
        let mut args = HashMap::new();
        args.insert("v".to_string(), Rc::new(KObject::Number(7.0)));

        let result = body(scope, &mut sched, ArgumentBundle { args }).expect_value("value_pass");

        assert!(matches!(result, KObject::Number(n) if *n == 7.0));
    }
}
