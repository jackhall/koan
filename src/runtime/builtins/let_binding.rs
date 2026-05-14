use crate::runtime::model::{Argument, ExpressionSignature, KObject, KType, SignatureElement};
use crate::runtime::machine::{ArgumentBundle, BodyResult, KError, KErrorKind, Scope, SchedulerHandle};
use crate::ast::{ExpressionPart, KExpression};

use super::{err, register_builtin_with_pre_run};

/// `LET <name> = <value:Any>` — copies the bound value into an arena-allocated `KObject`,
/// inserts it under `name`, and returns that same arena reference. Compound values recurse
/// through `KObject::deep_clone`.
///
/// Two overloads share this body, differing only in the `name` slot's `KType`: `Identifier`
/// (the original lowercase-name path) and `TypeExprRef` (so `LET ModuleName = (...)` can
/// bind a name that classifies as a Type token under the parser's token-classification rules).
pub fn body<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let name = match bundle.get("name") {
        Some(KObject::KString(s)) => s.clone(),
        // The `TypeExprRef` overload routes through `KTypeValue(kt)` post-refactor; only
        // leaf-named variants are valid binder names. Structural shapes (`List<X>`,
        // function types, `Mu` / `RecursiveRef`) are rejected as `ShapeError`.
        Some(KObject::KTypeValue(t)) => match t {
            KType::List(_)
            | KType::Dict(_, _)
            | KType::KFunction { .. }
            | KType::Mu { .. }
            | KType::RecursiveRef(_) => {
                return err(KError::new(KErrorKind::ShapeError(format!(
                    "LET name must be a bare type name, got `{}`",
                    t.render(),
                ))));
            }
            _ => {
                // Bind-time rejection for `LET <Type-class> = <non-type>`. Blocklist —
                // not `value.ktype() != KType::TypeExprRef` — because the type-language
                // carriers `KModule` / `KSignature` / `StructType` / `TaggedUnionType`
                // report `Module` / `Signature` / `Type`, not `TypeExprRef`, and shipped
                // `LET IntOrdAbstract = (IntOrd :| OrderedSig)` patterns in `ascribe.rs`
                // depend on those being accepted. Stage 1.7's storage-routing flip will
                // subsume this once type-language carriers move to `bindings.types`.
                let resolved_name = t.name();
                let value = match bundle.get("value") {
                    Some(v) => v,
                    None => return err(KError::new(KErrorKind::MissingArg("value".to_string()))),
                };
                if matches!(
                    value.ktype(),
                    KType::Number | KType::Str | KType::Bool | KType::Null
                        | KType::List(_) | KType::Dict(_, _)
                ) {
                    return err(KError::new(KErrorKind::TypeClassBindingExpectsType {
                        name: resolved_name,
                        got: value.ktype(),
                    }));
                }
                resolved_name
            }
        },
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
    use crate::runtime::builtins::default_scope;
    use crate::runtime::builtins::test_support::run_root_bare;
    use crate::runtime::model::KObject;
    use crate::runtime::machine::{ArgumentBundle, BodyResult};
    use crate::runtime::machine::execute::Scheduler;
    use crate::ast::{ExpressionPart, KExpression, KLiteral};

    #[test]
    fn let_inserts_binding_into_scope() {
        use crate::runtime::machine::RuntimeArena;
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
        let data = scope.bindings().data();
        let entry = data.get("x").expect("expected binding 'x'");
        assert!(matches!(entry, KObject::Number(n) if *n == 42.0));
    }

    /// Smoke test for LET's pre_run extractor: structural extraction of `parts[1]`
    /// returns the bound name without requiring sub-dispatches.
    #[test]
    fn pre_run_extracts_let_name() {
        use crate::parse::parse;
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
        use crate::runtime::machine::RuntimeArena;
        use crate::runtime::machine::execute::Scheduler;
        use crate::runtime::builtins::default_scope;
        use crate::parse::parse;
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let mut sched = Scheduler::new();
        let exprs = parse("LET hello = 1").unwrap();
        for e in exprs { sched.add_dispatch(e, scope); }
        sched.execute().unwrap();
        // After execute, placeholders should not contain "hello" — bind_value cleared it.
        assert!(scope.bindings().placeholders().get("hello").is_none());
        assert!(matches!(scope.lookup("hello"), Some(KObject::Number(n)) if *n == 1.0));
    }

    /// Phase 3: `LET T = T` is a trivially cyclic alias — the RHS references the binder
    /// itself. The dispatcher detects the placeholder-points-at-self condition and
    /// surfaces a structured `ShapeError` rather than parking the sub-Dispatch on its own
    /// ancestor (which would deadlock).
    #[test]
    fn let_t_cycle_errors() {
        use crate::runtime::machine::RuntimeArena;
        use crate::runtime::machine::execute::Scheduler;
        use crate::runtime::machine::KErrorKind;
        use crate::runtime::builtins::default_scope;
        use crate::parse::parse;
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let mut sched = Scheduler::new();
        let exprs = parse("LET Ty = Ty").unwrap();
        let mut ids = Vec::new();
        for e in exprs {
            ids.push(sched.add_dispatch(e, scope));
        }
        sched.execute().expect("execute does not surface per-slot errors");
        let res = sched.read_result(ids[0]);
        match res {
            Err(e) => assert!(
                matches!(&e.kind, KErrorKind::ShapeError(msg) if msg.contains("cycle")),
                "expected ShapeError mentioning cycle, got {e}",
            ),
            Ok(v) => panic!("expected cycle error, got value {:?}", v.ktype()),
        }
    }

    #[test]
    fn dispatch_let_expression() {
        use crate::runtime::machine::RuntimeArena;
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
        let data = scope.bindings().data();
        let entry = data.get("x").expect("expected binding 'x'");
        assert!(matches!(entry, KObject::Number(n) if *n == 42.0));
    }

    /// Stage 1.6: `LET Foo = 1` — Type-class LHS with a non-type RHS. The bind-time
    /// check fires before the value reaches storage, producing a structured
    /// `TypeClassBindingExpectsType` rather than the downstream `UnboundName` /
    /// `ShapeError` the old "bind silently" path eventually surfaced.
    #[test]
    fn let_type_class_with_non_type_value_errors() {
        use crate::runtime::machine::RuntimeArena;
        use crate::runtime::machine::KErrorKind;
        use crate::runtime::model::KType;
        use crate::parse::parse;
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let mut sched = Scheduler::new();
        let exprs = parse("LET Foo = 1").unwrap();
        let mut ids = Vec::new();
        for e in exprs {
            ids.push(sched.add_dispatch(e, scope));
        }
        sched.execute().expect("execute does not surface per-slot errors");
        let res = sched.read_result(ids[0]);
        match res {
            Err(e) => assert!(
                matches!(
                    &e.kind,
                    KErrorKind::TypeClassBindingExpectsType { name, got }
                        if name == "Foo" && matches!(got, KType::Number),
                ),
                "expected TypeClassBindingExpectsType {{ name: \"Foo\", got: Number }}, got {e}",
            ),
            Ok(v) => panic!("expected bind-time error, got value {:?}", v.ktype()),
        }
    }

    /// Stage 1.6: `LET Foo = Number` — Type-class LHS with a type RHS. Storage stays
    /// in `data` for now (stage 1.7 will flip routing to `bindings.types`). Regression
    /// guard that the blocklist doesn't reject the good case.
    #[test]
    fn let_type_class_with_type_value_still_binds() {
        use crate::runtime::machine::RuntimeArena;
        use crate::runtime::model::KType;
        use crate::parse::parse;
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let mut sched = Scheduler::new();
        let exprs = parse("LET Foo = Number").unwrap();
        let mut ids = Vec::new();
        for e in exprs {
            ids.push(sched.add_dispatch(e, scope));
        }
        sched.execute().expect("execute does not surface per-slot errors");
        let res = sched.read_result(ids[0]);
        assert!(res.is_ok(), "expected bind to succeed, got {:?}", res.err());
        let data = scope.bindings().data();
        let entry = data.get("Foo").expect("expected binding 'Foo'");
        assert!(
            matches!(entry, KObject::KTypeValue(t) if matches!(t, KType::Number)),
            "expected KTypeValue(Number), got {:?}",
            entry.ktype(),
        );
    }

    /// Stage 1.6: `LET foo = 1` (lowercase, Identifier overload) is untouched by
    /// the new check — it doesn't go through the `KTypeValue(_)` arm at all.
    #[test]
    fn let_identifier_lhs_with_non_type_still_binds() {
        use crate::runtime::machine::RuntimeArena;
        use crate::parse::parse;
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let mut sched = Scheduler::new();
        let exprs = parse("LET foo = 1").unwrap();
        let mut ids = Vec::new();
        for e in exprs {
            ids.push(sched.add_dispatch(e, scope));
        }
        sched.execute().expect("execute does not surface per-slot errors");
        let res = sched.read_result(ids[0]);
        assert!(res.is_ok(), "expected bind to succeed, got {:?}", res.err());
        let data = scope.bindings().data();
        let entry = data.get("foo").expect("expected binding 'foo'");
        assert!(
            matches!(entry, KObject::Number(n) if *n == 1.0),
            "expected Number(1.0), got {:?}",
            entry.ktype(),
        );
    }

    /// Stage 1.6: `LET List<Number> = 1` — parameterized binder name is rejected by
    /// the structural-shape check, which fires before the primitive blocklist.
    /// Regression guard for ordering.
    #[test]
    fn let_parameterized_type_lhs_still_shape_errors() {
        use crate::runtime::machine::RuntimeArena;
        use crate::runtime::machine::KErrorKind;
        use crate::parse::parse;
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let mut sched = Scheduler::new();
        let exprs = parse("LET List<Number> = 1").unwrap();
        let mut ids = Vec::new();
        for e in exprs {
            ids.push(sched.add_dispatch(e, scope));
        }
        sched.execute().expect("execute does not surface per-slot errors");
        let res = sched.read_result(ids[0]);
        match res {
            Err(e) => assert!(
                matches!(&e.kind, KErrorKind::ShapeError(_)),
                "expected ShapeError, got {e}",
            ),
            Ok(v) => panic!("expected shape error, got value {:?}", v.ktype()),
        }
    }

    /// Stage 1.6: `LET Foo = "hello"` — confirms the blocklist covers `Str`, not just
    /// `Number`. Same diagnostic shape as the Number case.
    #[test]
    fn let_type_class_with_string_value_errors() {
        use crate::runtime::machine::RuntimeArena;
        use crate::runtime::machine::KErrorKind;
        use crate::runtime::model::KType;
        use crate::parse::parse;
        let arena = RuntimeArena::new();
        let scope = default_scope(&arena, Box::new(std::io::sink()));
        let mut sched = Scheduler::new();
        let exprs = parse("LET Foo = \"hello\"").unwrap();
        let mut ids = Vec::new();
        for e in exprs {
            ids.push(sched.add_dispatch(e, scope));
        }
        sched.execute().expect("execute does not surface per-slot errors");
        let res = sched.read_result(ids[0]);
        match res {
            Err(e) => assert!(
                matches!(
                    &e.kind,
                    KErrorKind::TypeClassBindingExpectsType { name, got }
                        if name == "Foo" && matches!(got, KType::Str),
                ),
                "expected TypeClassBindingExpectsType {{ name: \"Foo\", got: Str }}, got {e}",
            ),
            Ok(v) => panic!("expected bind-time error, got value {:?}", v.ktype()),
        }
    }
}
