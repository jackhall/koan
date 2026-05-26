use crate::machine::model::types::UserTypeKind;
use crate::machine::model::{KObject, KType};
use crate::machine::{ArgumentBundle, BodyResult, Scope, ScopeId, SchedulerHandle};

use crate::builtins::err;

/// `TYPE_CONSTRUCTOR <param:TypeExprRef>` → `TypeExprRef` carrying a *template*
/// `KType::UserType { kind: UserTypeKind::TypeConstructor { param_names: vec![<param>] }, .. }`
/// with `scope_id: 0` and a placeholder `name` (`"_typeconstructor"`). The returned value
/// is a declaration template — `ascribe.rs:body_opaque` re-mints a fresh per-call
/// `scope_id` and the binding's slot name when the surrounding SIG is opaquely ascribed,
/// mirroring how `kind: Module` abstract-type slots get minted today. Stage 2 ships
/// arity-1 only; the `param_names` slot carries exactly one entry.
///
/// The `param` slot is read through `require_ktype` — the dispatcher has already resolved
/// either a bare `Type` token or a parameterized leaf into a `KTypeValue(_)`; we surface
/// its `name()` as the constructor's parameter symbol.
pub fn body<'a>(
    scope: &'a Scope<'a>,
    _sched: &mut dyn SchedulerHandle<'a>,
    bundle: ArgumentBundle<'a>,
) -> BodyResult<'a> {
    let param_kt = match bundle.require_ktype("param") {
        Ok(t) => t.clone(),
        Err(e) => return err(e),
    };
    // The parameter symbol is the bare Type-token name (`T`, `Elt`, ...). Structural
    // or parameterized shapes are rejected — `(TYPE_CONSTRUCTOR List<Number>)` would
    // be meaningless as a quantifier symbol.
    let param = param_kt.name();
    BodyResult::Value(
        scope.arena.alloc(KObject::KTypeValue(KType::UserType {
            kind: UserTypeKind::TypeConstructor { param_names: vec![param] },
            scope_id: ScopeId::SENTINEL,
            name: "_typeconstructor".into(),
        })),
    )
}

#[cfg(test)]
mod tests {
    use crate::builtins::test_support::{parse_one, run, run_one, run_root_silent};
    use crate::machine::execute::Scheduler;
    use crate::machine::model::types::UserTypeKind;
    use crate::machine::model::{KObject, KType};
    use crate::machine::{BindingIndex, RuntimeArena, ScopeId};

    // ---------- Module-system stage 2 Workstream B: TYPE_CONSTRUCTOR builtin ----------

    /// `(TYPE_CONSTRUCTOR Type)` returns a `KTypeValue` wrapping a template
    /// `KType::UserType { kind: UserTypeKind::TypeConstructor { param_names: ["T"] }, .. }`
    /// with the sentinel placeholder name (`_typeconstructor`) and `scope_id: 0`. The
    /// ascription site re-mints with the slot's declared name and a fresh per-call
    /// `scope_id`; this test just pins the template shape the builtin returns.
    #[test]
    fn type_constructor_builtin_returns_ktype_value() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let result = run_one(scope, parse_one("TYPE_CONSTRUCTOR Type"));
        match result {
            KObject::KTypeValue(kt) => match kt {
                KType::UserType { kind: UserTypeKind::TypeConstructor { param_names }, scope_id, name } => {
                    assert_eq!(*param_names, vec!["Type".to_string()]);
                    assert_eq!(*scope_id, ScopeId::SENTINEL);
                    assert_eq!(name, "_typeconstructor");
                }
                other => panic!("expected UserType(TypeConstructor), got {:?}", other),
            },
            other => panic!("expected KTypeValue, got {:?}", other.ktype()),
        }
    }

    /// `SIG Monad = ((LET Wrap = (TYPE_CONSTRUCTOR Type)))` parses and binds. The SIG's
    /// decl-scope carries a `KType::UserType { kind: TypeConstructor { .. }, .. }` template
    /// in `bindings.types` under `Wrap`. Pins the LET-routing + register_type path.
    #[test]
    fn sig_declares_higher_kinded_slot() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(scope, "SIG Monad = ((LET Wrap = (TYPE_CONSTRUCTOR Type)))");
        let s = match scope.bindings().data().get("Monad").map(|(o, _)| *o) {
            Some(KObject::KTypeValue(KType::Signature(s))) => *s,
            _ => panic!("Monad must bind a KSignature"),
        };
        let wrap_kt: &KType = s.decl_scope().bindings().expect_type("Wrap");
        match wrap_kt {
            KType::UserType { kind: UserTypeKind::TypeConstructor { param_names }, .. } => {
                assert_eq!(*param_names, vec!["Type".to_string()]);
            }
            other => panic!("expected UserType(TypeConstructor) under Wrap, got {:?}", other),
        }
    }

    /// FN-def whose return type is `Wrap<Number>` against a root-scope-bound
    /// TypeConstructor `Wrap`. Pins the dispatch path: `resolve_for` turns the
    /// parameterized type into a `TypeNameRef` carrier, `elaborate_type_expr` runs
    /// the new ConstructorApply arm, and the FN's stored signature carries a
    /// `KType::ConstructorApply { ctor: Wrap, args: [Number] }`. Isolates the path
    /// from SIG-body forward-reference parking (covered by `monad_signature_smoke`).
    /// Root-scope LET is unchanged by the VAL refactor — only SIG-body lowercase
    /// LETs migrated.
    #[test]
    fn fn_return_type_constructor_apply_root_scope() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        scope.register_type(
            "Wrap".into(),
            KType::UserType {
                kind: UserTypeKind::TypeConstructor { param_names: vec!["Type".into()] },
                scope_id: ScopeId::from_raw(0, 0xC0DE),
                name: "Wrap".into(),
            },
            BindingIndex::BUILTIN,
        );
        let mut sched = Scheduler::new();
        let id = sched.add_dispatch(
            parse_one("LET pure = (FN (PURE a :Number) -> :(Wrap Number) = (1))"),
            scope,
        );
        sched.execute().expect("scheduler should run");
        match sched.read_result(id) {
            Ok(_) => {}
            Err(e) => panic!("FN with :(Wrap Number) return failed: {}", e),
        }
        // Verify the FN's return type is ConstructorApply<Wrap, [Number]>.
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

    /// Module-system stage 2 Workstream B2: end-to-end smoke test for the monad-shaped
    /// signature. `SIG Monad = ((LET Wrap = (TYPE_CONSTRUCTOR Type)) (VAL pure:
    /// Function<(Number) -> Wrap<Number>>))` parses, the SIG body's VAL slot elaborates
    /// `Function<(Number) -> Wrap<Number>>` through the existing `Function` arm in
    /// `elaborate_type_expr` and the inner `Wrap<Number>` through the new
    /// `ConstructorApply` arm. The resulting `pure` member is bound under the SIG's
    /// decl-scope as a `KTypeValue(KFunction { args, ret: ConstructorApply{Wrap, …} })`
    /// carrier (the post-VAL slot shape; pre-VAL the slot bound the ascription-by-example
    /// FN value directly). Load-bearing for `monadic-side-effects.md`.
    ///
    /// `Number` is used as the parameter type rather than `T` because koan's token
    /// classification rejects single-letter Type tokens (needs ≥1 lowercase). The
    /// roadmap-decided surface form `(TYPE_CONSTRUCTOR T)` is conceptual; the runtime
    /// param symbol is whatever Type-classified token the user writes (here `Type`,
    /// a builtin meta-type name).
    ///
    /// SIG-body order matters: `LET Wrap` precedes the `VAL pure` slot so the inner
    /// `Wrap<Number>` resolves synchronously against the SIG decl_scope's
    /// `bindings.types["Wrap"]` entry. VAL's structural-`TypeNameRef` arm elaborates
    /// synchronously and surfaces a ShapeError on park — see `val_decl.rs`'s body for
    /// the rationale (no safe park route for structural shapes today).
    #[test]
    fn monad_signature_smoke() {
        use crate::parse::parse;
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        let src = "SIG Monad = ((LET Wrap = (TYPE_CONSTRUCTOR Type)) \
             (VAL pure :(Function (Number) -> :(Wrap Number))))";
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
        // The SIG must have bound — pull it out of scope and walk its decl_scope.
        let s = match scope.bindings().data().get("Monad").map(|(o, _)| *o) {
            Some(KObject::KTypeValue(KType::Signature(s))) => *s,
            other => panic!("Monad must bind a KSignature, got {:?}", other.map(|o| o.ktype())),
        };
        // `Wrap` lives in the SIG's `bindings.types` as a TypeConstructor template.
        let wrap_kt: &KType = s.decl_scope().bindings().expect_type("Wrap");
        assert!(matches!(
            wrap_kt,
            KType::UserType { kind: UserTypeKind::TypeConstructor { .. }, .. }
        ));
        // `pure` is bound in the SIG's `bindings.data` as a `KTypeValue` carrying the
        // declared `Function<(Number) -> Wrap<Number>>` type (post-VAL slot shape).
        // The inner `Wrap<Number>` elaborated against the SIG decl_scope as a
        // `ConstructorApply { ctor: Wrap, args: [Number] }`.
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
                        assert!(matches!(
                            ctor.as_ref(),
                            KType::UserType { kind: UserTypeKind::TypeConstructor { .. }, .. }
                        ), "ConstructorApply.ctor must be a TypeConstructor, got {:?}", ctor);
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

    /// `(M.Wrap)` after opaque ascription resolves through the new module's
    /// `type_members` to the per-call-minted constructor variant. Pins the ATTR path's
    /// flow: `attr.rs` routes `Foo.Wrap` through `type_members` lookup, and the new
    /// `UserTypeKind::TypeConstructor` variant flows through the existing
    /// `KType::UserType` arm unchanged.
    #[test]
    fn module_attr_access_returns_type_constructor() {
        let arena = RuntimeArena::new();
        let scope = run_root_silent(&arena);
        run(
            scope,
            "SIG MonadSig = ((LET Wrap = (TYPE_CONSTRUCTOR Type)))\n\
             MODULE IntList = ((LET Wrap = Number))\n\
             LET Mo = (IntList :| MonadSig)",
        );
        // Mo's type_members must carry a TypeConstructor slot under `Wrap`.
        let mo = match scope.bindings().data().get("Mo").map(|(o, _)| *o) {
            Some(KObject::KTypeValue(KType::Module { module: m, .. })) => *m,
            other => panic!("Mo should be a module, got {:?}", other.map(|o| o.ktype())),
        };
        let wrap_t = mo.type_members.borrow().get("Wrap").cloned();
        match wrap_t {
            Some(KType::UserType { kind: UserTypeKind::TypeConstructor { param_names }, name, .. }) => {
                assert_eq!(name, "Wrap");
                // The per-call mint carries the SIG's declared param-name list.
                assert_eq!(param_names, vec!["Type".to_string()]);
            }
            other => panic!(
                "expected TypeConstructor in type_members[Wrap], got {:?}",
                other,
            ),
        }
    }
}
