use crate::runtime::model::{Argument, ExpressionSignature, KObject, KType, Parseable, SignatureElement};
use crate::runtime::machine::{ArgumentBundle, BodyResult, KError, KErrorKind, Scope, SchedulerHandle};

use super::{err, register_builtin};

/// `<v:Identifier>` or `<v:TypeExprRef>` — single-part expression containing one name token.
/// Looks `v` up via `Scope::lookup` (which walks the `outer` chain) and returns the bound
/// `KObject`, or `KError::UnboundName` if unbound at every level. Lets a parens-wrapped name
/// (`(some_var)`, `(IntOrd)`) — or an auto-wrap of the same — dispatch and resolve to its
/// current value. The TypeExprRef overload only accepts bare leaves (`params: None`);
/// parameterized type expressions like `List<Number>` are structural type-syntax, not
/// look-up targets.
pub fn body<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let v = match bundle.get("v") {
        Some(KObject::KString(s)) => s.clone(),
        Some(KObject::TypeExprValue(t)) => {
            if !matches!(t.params, crate::ast::TypeParams::None) {
                return err(KError::new(KErrorKind::ShapeError(format!(
                    "value_lookup: parameterized type expression `{}` is not a value-lookup target",
                    t.render()
                ))));
            }
            t.name.clone()
        }
        other => {
            return err(KError::new(KErrorKind::TypeMismatch {
                arg: "v".to_string(),
                expected: "KString or TypeExprValue".to_string(),
                got: match other {
                    Some(o) => o.summarize(),
                    None => "(missing)".to_string(),
                },
            }));
        }
    };
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
    register_builtin(
        scope,
        "value_lookup",
        ExpressionSignature {
            return_type: KType::Any,
            elements: vec![
                SignatureElement::Argument(Argument { name: "v".into(), ktype: KType::TypeExprRef }),
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
    use crate::runtime::builtins::test_support::run_root_bare;
    use crate::runtime::model::KObject;
    use crate::runtime::machine::{ArgumentBundle, BodyResult, KError, KErrorKind, RuntimeArena, Scope};
    use crate::runtime::machine::execute::Scheduler;

    fn run_body<'a>(
        scope: &'a Scope<'a>,
        bundle: ArgumentBundle<'a>,
    ) -> &'a KObject<'a> {
        let mut sched = Scheduler::new();
        match body(scope, &mut sched, bundle) {
            BodyResult::Value(v) => v,
            BodyResult::Tail { .. } => panic!("value_lookup should not produce a Tail"),
            BodyResult::DeferTo(_) => panic!("value_lookup should not produce a DeferTo"),
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
        let scope = run_root_bare(&arena);
        let bound = arena.alloc_object(KObject::Number(42.0));
        scope.bind_value("foo".to_string(), bound).unwrap();

        let mut args = HashMap::new();
        args.insert("v".to_string(), Rc::new(KObject::KString("foo".into())));

        let result = run_body(scope, ArgumentBundle { args });

        assert!(matches!(result, KObject::Number(n) if *n == 42.0));
    }

    #[test]
    fn value_lookup_unbound_returns_error() {
        let arena = RuntimeArena::new();
        let scope = run_root_bare(&arena);
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
            BodyResult::DeferTo(_) => "DeferTo",
            BodyResult::Err(_) => "Err",
        }
    }

    #[test]
    fn value_lookup_walks_outer_scope() {
        let arena = RuntimeArena::new();
        let outer = run_root_bare(&arena);
        let bound = arena.alloc_object(KObject::Number(7.0));
        outer.bind_value("from_outer".to_string(), bound).unwrap();

        let inner = arena.alloc_scope(outer.child_for_call());

        let mut args = HashMap::new();
        args.insert("v".to_string(), Rc::new(KObject::KString("from_outer".into())));

        let result = run_body(inner, ArgumentBundle { args });

        assert!(matches!(result, KObject::Number(n) if *n == 7.0));
    }
}
