use crate::runtime::model::{Argument, ExpressionSignature, KObject, KType, Parseable, SignatureElement};
use crate::runtime::machine::{ArgumentBundle, BodyResult, KError, KErrorKind, Scope, SchedulerHandle};

use super::{err, register_builtin};

/// `<v:Identifier>` — single-part expression containing one identifier-classed name token.
/// Looks `v` up via `Scope::lookup` (walking `data → placeholders → outer`) and returns
/// the bound `KObject`, or `KError::UnboundName` if unbound at every level. Lets a
/// parens-wrapped name (`(some_var)`) — or an auto-wrap of the same — dispatch and
/// resolve to its current value.
pub fn body_identifier<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let v = match bundle.get("v") {
        Some(KObject::KString(s)) => s.clone(),
        other => {
            return err(KError::new(KErrorKind::TypeMismatch {
                arg: "v".to_string(),
                expected: "KString".to_string(),
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

/// `<v:TypeExprRef>` — single-part expression containing one type-classed name token.
/// Resolves the name in two stages, mirroring the same `resolve_type` → `lookup`
/// fall-through `elaborate_type_expr` uses for bare-leaf names:
///
/// 1. Try `Scope::resolve_type` first. On hit (the builtin type names registered via
///    `Scope::register_type`, which post-stage-1.4 live in `bindings.types`), synthesize
///    a per-lookup `KObject::KTypeValue(kt.clone())` carrier so the value sits in the
///    same dispatch transport every other body consumes. This is the *sole*
///    `KObject::KTypeValue` synthesis site post-stage-1.5 — the transient
///    `Scope::resolve` fallback that previously fabricated the wrap on demand is gone.
/// 2. On a `resolve_type` miss, fall through to `scope.lookup`. Type-classed *names*
///    whose nominal value-side bindings still live in `data` (`KObject::KModule` from
///    `MODULE Foo`, `KObject::StructType` from `STRUCT Foo`, `KObject::TaggedUnionType`
///    from `UNION Foo`, `KObject::KSignature` from `SIG Foo`) get found here. Those
///    carriers don't dual-write to `bindings.types` until stage 3 mints
///    `KType::UserType` and adds the dual-write surface.
/// 3. On a miss everywhere, `UnboundName`.
///
/// The per-lookup `alloc_object` cost in stage 1 is bounded by program-source type
/// references (not hot-loop iterations) and pays for the binding-home invariant (type
/// names live in `bindings.types`, identifier names live in `bindings.data`, no overlap
/// in this body).
///
/// Structural type-syntax (`List<X>`, function types, `Mu` / `RecursiveRef`) is rejected
/// up front with `ShapeError` — only leaf-named variants are lookup targets. The
/// rejection runs on the incoming `KTypeValue` bundle slot *before* either lookup, so
/// the structural-shape error fires identically to the pre-migration behavior.
pub fn body_type_expr<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let name = match bundle.get("v") {
        Some(KObject::KTypeValue(t)) => match t {
            KType::List(_)
            | KType::Dict(_, _)
            | KType::KFunction { .. }
            | KType::Mu { .. }
            | KType::RecursiveRef(_) => {
                return err(KError::new(KErrorKind::ShapeError(format!(
                    "value_lookup: parameterized type expression `{}` is not a value-lookup target",
                    t.render()
                ))));
            }
            _ => t.name(),
        },
        // Stage-2 carrier: a bare-leaf type token whose name isn't in the builtin table
        // (`Foo` after `STRUCT Foo = …`, etc.) lands here from auto-wrap's
        // `Type(t) → (Type(t))` sub-Dispatch. Reject parameterized shapes the same way
        // the `KTypeValue` arm does; otherwise the surface name (`t.name`) is the
        // lookup target.
        Some(KObject::TypeNameRef(t, _)) => match &t.params {
            crate::ast::TypeParams::List(_) | crate::ast::TypeParams::Function { .. } => {
                return err(KError::new(KErrorKind::ShapeError(format!(
                    "value_lookup: parameterized type expression `{}` is not a value-lookup target",
                    t.render()
                ))));
            }
            crate::ast::TypeParams::None => t.name.clone(),
        },
        other => {
            return err(KError::new(KErrorKind::TypeMismatch {
                arg: "v".to_string(),
                expected: "KTypeValue".to_string(),
                got: match other {
                    Some(o) => o.summarize(),
                    None => "(missing)".to_string(),
                },
            }));
        }
    };
    if let Some(kt) = scope.resolve_type(&name) {
        let obj = scope.arena.alloc_object(KObject::KTypeValue(kt.clone()));
        return BodyResult::Value(obj);
    }
    match scope.lookup(&name) {
        Some(obj) => BodyResult::Value(obj),
        None => err(KError::new(KErrorKind::UnboundName(name))),
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
        body_identifier,
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
        body_type_expr,
    );
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::rc::Rc;

    use super::{body_identifier, body_type_expr};
    use crate::runtime::builtins::test_support::run_root_bare;
    use crate::runtime::model::{KObject, KType};
    use crate::runtime::machine::{ArgumentBundle, BodyResult, KError, KErrorKind, RuntimeArena, Scope};
    use crate::runtime::machine::execute::Scheduler;

    fn run_body<'a>(
        scope: &'a Scope<'a>,
        bundle: ArgumentBundle<'a>,
    ) -> &'a KObject<'a> {
        let mut sched = Scheduler::new();
        match body_identifier(scope, &mut sched, bundle) {
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
        body_identifier(scope, &mut sched, bundle)
    }

    fn run_body_type_expr<'a>(
        scope: &'a Scope<'a>,
        bundle: ArgumentBundle<'a>,
    ) -> BodyResult<'a> {
        let mut sched = Scheduler::new();
        body_type_expr(scope, &mut sched, bundle)
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

    /// `body_type_expr` consults `Scope::resolve_type` (the post-1.5 type-side binding
    /// home) and synthesizes the `KObject::KTypeValue` dispatch transport on hit. The
    /// incoming bundle slot's `KTypeValue` only carries the surface leaf name via
    /// `name()`; the result is the `KType` stored under that name in `bindings.types`.
    #[test]
    fn value_lookup_type_expr_resolves_via_resolve_type() {
        let arena = RuntimeArena::new();
        let scope = run_root_bare(&arena);
        // Register the type under the name `Number` — `KType::Number.name()` is "Number",
        // so the body's `resolve_type("Number")` walks the types map and returns the
        // arena-allocated entry.
        scope.register_type("Number".into(), KType::Number);
        let mut args = HashMap::new();
        args.insert(
            "v".to_string(),
            Rc::new(KObject::KTypeValue(KType::Number)),
        );
        let result = run_body_type_expr(scope, ArgumentBundle { args });
        match result {
            BodyResult::Value(KObject::KTypeValue(kt)) => {
                assert!(matches!(kt, KType::Number), "expected Number, got {kt:?}");
            }
            other => panic!(
                "expected Value(KTypeValue(Number)), got {:?}",
                error_kind_name(&other)
            ),
        }
    }

    /// Structural type shapes are rejected before any `resolve_type` lookup — the same
    /// `ShapeError` the pre-migration body produced.
    #[test]
    fn value_lookup_type_expr_rejects_parameterized_shapes() {
        let arena = RuntimeArena::new();
        let scope = run_root_bare(&arena);
        let mut args = HashMap::new();
        args.insert(
            "v".to_string(),
            Rc::new(KObject::KTypeValue(KType::List(Box::new(KType::Number)))),
        );
        let result = run_body_type_expr(scope, ArgumentBundle { args });
        match result {
            BodyResult::Err(KError { kind: KErrorKind::ShapeError(msg), .. }) => {
                assert!(
                    msg.contains("parameterized type expression"),
                    "expected ShapeError about parameterized type, got `{msg}`",
                );
            }
            other => panic!(
                "expected ShapeError on parameterized lookup, got {:?}",
                error_kind_name(&other)
            ),
        }
    }

    /// An unbound type-token name surfaces as `UnboundName`.
    #[test]
    fn value_lookup_type_expr_unbound_returns_error() {
        let arena = RuntimeArena::new();
        let scope = run_root_bare(&arena);
        // No `register_type` call — the bare runtime scope has no types map entries.
        let mut args = HashMap::new();
        args.insert(
            "v".to_string(),
            Rc::new(KObject::KTypeValue(KType::Number)),
        );
        let result = run_body_type_expr(scope, ArgumentBundle { args });
        match result {
            BodyResult::Err(KError { kind: KErrorKind::UnboundName(name), .. }) => {
                assert_eq!(name, "Number");
            }
            other => panic!(
                "expected UnboundName on missing type, got {:?}",
                error_kind_name(&other)
            ),
        }
    }
}
