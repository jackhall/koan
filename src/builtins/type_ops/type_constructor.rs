use crate::machine::model::types::UserTypeKind;
use crate::machine::model::{KObject, KType};
use crate::machine::{ArgumentBundle, BodyResult, SchedulerHandle, Scope, ScopeId};

use crate::builtins::err;

/// `TEMPLATE <param:TypeExprRef>` → `TypeExprRef` carrying a template
/// `KType::UserType { kind: UserTypeKind::TypeConstructor { param_names, .. }, .. }`
/// with `ScopeId::SENTINEL` and a placeholder `name` (`"_typeconstructor"`). The
/// surrounding opaque ascription (`ascribe.rs:body_opaque`) re-mints both with the
/// binding's slot name and a per-call `scope_id`. Arity-1 only.
pub fn body<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let param_kt = match bundle.require_ktype("param") {
        Ok(t) => t.clone(),
        Err(e) => return err(e),
    };
    let param = param_kt.name();
    BodyResult::Value(scope.arena.alloc(KObject::KTypeValue(KType::UserType {
        // Abstract higher-kinded SIG slot — not a constructible union, so the
        // schema payload is empty (equality ignores it anyway).
        kind: UserTypeKind::TypeConstructor {
            schema: std::rc::Rc::new(std::collections::HashMap::new()),
            param_names: vec![param],
        },
        scope_id: ScopeId::SENTINEL,
        name: "_typeconstructor".into(),
    })))
}

#[cfg(test)]
mod tests {
    use crate::builtins::test_support::{parse_one, run, run_one, run_root_silent};
    use crate::machine::execute::Scheduler;
    use crate::machine::model::types::UserTypeKind;
    use crate::machine::model::{KObject, KType};
    use crate::machine::{BindingIndex, RuntimeArena, ScopeId};

    /// Pins the template shape the builtin returns before opaque ascription re-mints it.
    #[test]
    fn type_constructor_builtin_returns_ktype_value() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let result = run_one(scope, parse_one("TEMPLATE Type"));
        match result {
            KObject::KTypeValue(kt) => match kt {
                KType::UserType {
                    kind: UserTypeKind::TypeConstructor { param_names, .. },
                    scope_id,
                    name,
                } => {
                    assert_eq!(*param_names, vec!["Type".to_string()]);
                    assert_eq!(*scope_id, ScopeId::SENTINEL);
                    assert_eq!(name, "_typeconstructor");
                }
                other => panic!("expected UserType(TypeConstructor), got {:?}", other),
            },
            other => panic!("expected KTypeValue, got {:?}", other.ktype()),
        }
    }

    /// Pins the LET-routing + `register_type` path for a higher-kinded SIG slot.
    #[test]
    fn sig_declares_higher_kinded_slot() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "SIG Monad = ((LET Wrap = (TEMPLATE Type)))");
        let s = match scope.resolve_type("Monad") {
            Some(KType::Signature { sig, .. }) => *sig,
            _ => panic!("Monad must bind a Signature KType"),
        };
        let wrap_kt: &KType = s.decl_scope().bindings().expect_type("Wrap");
        match wrap_kt {
            KType::UserType {
                kind: UserTypeKind::TypeConstructor { param_names, .. },
                ..
            } => {
                assert_eq!(*param_names, vec!["Type".to_string()]);
            }
            other => panic!(
                "expected UserType(TypeConstructor) under Wrap, got {:?}",
                other
            ),
        }
    }

    /// Pins the dispatch path for an FN return type `:(Number AS Wrap)` against a
    /// root-scope-bound TypeConstructor — the `AS` keyworded builtin lowers it to a
    /// `ConstructorApply` carrier.
    #[test]
    fn fn_return_type_constructor_apply_root_scope() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        scope.register_type(
            "Wrap".into(),
            KType::UserType {
                kind: UserTypeKind::TypeConstructor {
                    schema: std::rc::Rc::new(std::collections::HashMap::new()),
                    param_names: vec!["Type".into()],
                },
                scope_id: ScopeId::from_raw(0, 0xC0DE),
                name: "Wrap".into(),
            },
            BindingIndex::BUILTIN,
        );
        let mut sched = Scheduler::new();
        let id = sched.add_dispatch(
            parse_one("LET pure = (FN (PURE a :Number) -> :(Number AS Wrap) = (1))"),
            scope,
        );
        sched.execute().expect("scheduler should run");
        match sched.read_result(id) {
            Ok(_) => {}
            Err(e) => panic!("FN with :(Number AS Wrap) return failed: {}", e),
        }
        let pure = scope.bindings().expect_value("pure");
        let f = match pure {
            KObject::KFunction(f, _) => *f,
            other => panic!("pure not KFunction: {:?}", other.ktype()),
        };
        use crate::machine::model::ReturnType;
        match &f.signature.return_type {
            ReturnType::Resolved(KType::ConstructorApply { args, .. }) => {
                assert_eq!(*args, vec![KType::Number]);
            }
            other => panic!("expected Resolved(ConstructorApply), got {:?}", other),
        }
    }

    /// End-to-end smoke for a monad-shaped signature: `LET Wrap` precedes
    /// `VAL pure` so the inner `:(Number AS Wrap)` resolves synchronously against the
    /// SIG decl-scope's `bindings.types["Wrap"]` entry.
    #[test]
    fn monad_signature_smoke() {
        use crate::parse::parse;
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let src = "SIG Monad = ((LET Wrap = (TEMPLATE Type)) \
             (VAL pure :(FN (x :Number) -> :(Number AS Wrap))))";
        let exprs = parse(src).expect("parse should succeed");
        let mut sched = Scheduler::new();
        let mut ids = Vec::new();
        for expr in exprs {
            ids.push(sched.add_dispatch(expr, scope));
        }
        match sched.execute() {
            Ok(()) => {}
            Err(e) => panic!("scheduler errored: {}", e),
        }
        for (i, id) in ids.iter().enumerate() {
            if let Err(e) = sched.read_result(*id) {
                panic!("expr {} errored: {}", i, e);
            }
        }
        let s = match scope.resolve_type("Monad") {
            Some(KType::Signature { sig, .. }) => *sig,
            other => panic!("Monad must bind a Signature KType, got {:?}", other),
        };
        let wrap_kt: &KType = s.decl_scope().bindings().expect_type("Wrap");
        assert!(matches!(
            wrap_kt,
            KType::UserType {
                kind: UserTypeKind::TypeConstructor { .. },
                ..
            }
        ));
        let pure = s.decl_scope().bindings().expect_value("pure");
        let kt = match pure {
            KObject::KTypeValue(kt) => kt,
            other => panic!("pure must be a KTypeValue, got {:?}", other.ktype()),
        };
        match kt {
            KType::KFunction { args, ret } => {
                assert_eq!(*args, vec![KType::Number]);
                match ret.as_ref() {
                    KType::ConstructorApply { ctor, args } => {
                        assert!(
                            matches!(
                                ctor.as_ref(),
                                KType::UserType {
                                    kind: UserTypeKind::TypeConstructor { .. },
                                    ..
                                }
                            ),
                            "ConstructorApply.ctor must be a TypeConstructor, got {:?}",
                            ctor
                        );
                        assert_eq!(*args, vec![KType::Number]);
                    }
                    other => panic!(
                        "pure return type must be ConstructorApply(Wrap, [Number]), got {:?}",
                        other,
                    ),
                }
            }
            other => panic!("pure must be a Function type, got {:?}", other),
        }
    }

    /// `(M.Wrap)` after opaque ascription resolves through the module's
    /// `type_members` to the per-call-minted constructor variant.
    #[test]
    fn module_attr_access_returns_type_constructor() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "SIG MonadSig = ((LET Wrap = (TEMPLATE Type)))\n\
             MODULE IntList = ((LET Wrap = Number))\n\
             LET Mo = (IntList :| MonadSig)",
        );
        let mo = match scope.resolve_type("Mo") {
            Some(KType::Module { module: m, .. }) => *m,
            other => panic!("Mo should be a module identity in types, got {other:?}"),
        };
        let wrap_t = mo.type_members.borrow().get("Wrap").cloned();
        match wrap_t {
            Some(KType::UserType {
                kind: UserTypeKind::TypeConstructor { param_names, .. },
                name,
                ..
            }) => {
                assert_eq!(name, "Wrap");
                assert_eq!(param_names, vec!["Type".to_string()]);
            }
            other => panic!(
                "expected TypeConstructor in type_members[Wrap], got {:?}",
                other,
            ),
        }
    }
}
