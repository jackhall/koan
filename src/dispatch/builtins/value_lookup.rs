use crate::dispatch::kfunction::{
    Argument, ArgumentBundle, BodyResult, ExpressionSignature, KType, SchedulerHandle,
    SignatureElement,
};
use crate::dispatch::scope::Scope;
use crate::try_args;

use super::{null, register_builtin};

/// `<v:Identifier>` — single-part expression containing one name token. Looks `v` up via
/// `Scope::lookup` (which walks the `outer` chain) and returns the bound `KObject`, or `Null`
/// if unbound at every level. Lets a parens-wrapped name like `(some_var)` dispatch and
/// resolve to its current value.
pub fn body<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    try_args!(bundle, return null(); v: KString);
    match scope.lookup(&v) {
        Some(obj) => BodyResult::Value(obj),
        None => null(),
    }
}

pub fn register<'a>(scope: &'a Scope<'a>) {
    register_builtin(
        scope,
        "value_lookup",
        ExpressionSignature {
            return_type: KType::Any,
            elements: vec![
                SignatureElement::Argument(Argument { name: "v".into(), ktype: KType::Identifier }),
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
    use crate::dispatch::arena::RuntimeArena;
    use crate::dispatch::kfunction::{ArgumentBundle, BodyResult};
    use crate::dispatch::kobject::KObject;
    use crate::dispatch::scope::Scope;
    use crate::execute::scheduler::Scheduler;

    fn run_body<'a>(
        scope: &'a Scope<'a>,
        bundle: ArgumentBundle<'a>,
    ) -> &'a KObject<'a> {
        let mut sched = Scheduler::new();
        match body(scope, &mut sched, bundle) {
            BodyResult::Value(v) => v,
            BodyResult::Tail { .. } => panic!("value_lookup should not produce a Tail"),
        }
    }

    #[test]
    fn value_lookup_returns_binding() {
        let arena = RuntimeArena::new();
        let scope = arena.alloc_scope(Scope::run_root(&arena, None, Box::new(std::io::sink())));
        let bound = arena.alloc_object(KObject::Number(42.0));
        scope.data.borrow_mut().insert("foo".to_string(), bound);

        let mut args = HashMap::new();
        args.insert("v".to_string(), Rc::new(KObject::KString("foo".into())));

        let result = run_body(scope, ArgumentBundle { args });

        assert!(matches!(result, KObject::Number(n) if *n == 42.0));
    }

    #[test]
    fn value_lookup_unbound_returns_null() {
        let arena = RuntimeArena::new();
        let scope = arena.alloc_scope(Scope::run_root(&arena, None, Box::new(std::io::sink())));
        let mut args = HashMap::new();
        args.insert("v".to_string(), Rc::new(KObject::KString("missing".into())));

        let result = run_body(scope, ArgumentBundle { args });

        assert!(matches!(result, KObject::Null));
    }

    #[test]
    fn value_lookup_walks_outer_scope() {
        let arena = RuntimeArena::new();
        let outer = arena.alloc_scope(Scope::run_root(&arena, None, Box::new(std::io::sink())));
        let bound = arena.alloc_object(KObject::Number(7.0));
        outer.data.borrow_mut().insert("from_outer".to_string(), bound);

        let inner = arena.alloc_scope(outer.child_for_call());

        let mut args = HashMap::new();
        args.insert("v".to_string(), Rc::new(KObject::KString("from_outer".into())));

        let result = run_body(inner, ArgumentBundle { args });

        assert!(matches!(result, KObject::Number(n) if *n == 7.0));
    }
}
