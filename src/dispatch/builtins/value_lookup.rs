use crate::dispatch::runtime::{KError, KErrorKind};
use crate::dispatch::kfunction::{ArgumentBundle, BodyResult, SchedulerHandle};
use crate::dispatch::types::{Argument, ExpressionSignature, KType, SignatureElement};
use crate::dispatch::runtime::Scope;
use crate::try_args;

use super::{err, register_builtin};

/// `<v:Identifier>` — single-part expression containing one name token. Looks `v` up via
/// `Scope::lookup` (which walks the `outer` chain) and returns the bound `KObject`, or
/// `KError::UnboundName` if unbound at every level. Lets a parens-wrapped name like
/// `(some_var)` dispatch and resolve to its current value.
pub fn body<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    try_args!(bundle; v: KString);
    match scope.lookup(&v) {
        Some(obj) => BodyResult::Value(obj),
        None => err(KError::new(KErrorKind::UnboundName(v))),
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
    use crate::dispatch::runtime::RuntimeArena;
    use crate::dispatch::runtime::{KError, KErrorKind};
    use crate::dispatch::kfunction::{ArgumentBundle, BodyResult};
    use crate::dispatch::values::KObject;
    use crate::dispatch::runtime::Scope;
    use crate::execute::scheduler::Scheduler;

    fn run_body<'a>(
        scope: &'a Scope<'a>,
        bundle: ArgumentBundle<'a>,
    ) -> &'a KObject<'a> {
        let mut sched = Scheduler::new();
        match body(scope, &mut sched, bundle) {
            BodyResult::Value(v) => v,
            BodyResult::Tail { .. } => panic!("value_lookup should not produce a Tail"),
            BodyResult::Err(e) => panic!("value_lookup errored unexpectedly: {e}"),
        }
    }

    /// Like `run_body` but returns the `BodyResult` so error-path tests can pattern-match
    /// on the `Err` variant.
    fn run_body_result<'a>(
        scope: &'a Scope<'a>,
        bundle: ArgumentBundle<'a>,
    ) -> BodyResult<'a> {
        let mut sched = Scheduler::new();
        body(scope, &mut sched, bundle)
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
    fn value_lookup_unbound_returns_error() {
        let arena = RuntimeArena::new();
        let scope = arena.alloc_scope(Scope::run_root(&arena, None, Box::new(std::io::sink())));
        let mut args = HashMap::new();
        args.insert("v".to_string(), Rc::new(KObject::KString("missing".into())));

        let result = run_body_result(scope, ArgumentBundle { args });

        match result {
            BodyResult::Err(KError { kind: KErrorKind::UnboundName(name), .. }) => {
                assert_eq!(name, "missing");
            }
            other => panic!("expected UnboundName error, got {:?}", error_kind_name(&other)),
        }
    }

    fn error_kind_name(r: &BodyResult<'_>) -> &'static str {
        match r {
            BodyResult::Value(_) => "Value",
            BodyResult::Tail { .. } => "Tail",
            BodyResult::Err(_) => "Err",
        }
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
