use std::rc::Rc;

use crate::machine::model::types::{KKind, NominalMember, NominalSchema, RecursiveSet};
use crate::machine::model::KType;
use crate::machine::ScopeId;

/// `TEMPLATE <param:ProperType>` → `ProperType` carrying a template singleton
/// [`RecursiveSet`] of one [`KKind::TypeConstructor`] member with `ScopeId::SENTINEL`
/// and a placeholder `name` (`"_typeconstructor"`). The surrounding opaque ascription
/// (`ascribe.rs:body_opaque`) re-mints a fresh per-call singleton with the binding's slot
/// name and a per-call `scope_id`. Arity-1 only. Reads the `param` type cell from
/// `BodyCtx::args`, mints the template singleton [`RecursiveSet`], and returns it as a
/// `Carried::Type`.
pub fn body<'a>(
    ctx: &crate::machine::core::kfunction::action::BodyCtx<'a, '_>,
) -> crate::machine::core::kfunction::action::Action<'a> {
    use crate::machine::core::kfunction::action::{arg_held, arg_type, Action};
    use crate::machine::model::values::Held;
    use crate::machine::model::Carried;
    use crate::machine::{KError, KErrorKind};

    let param_kt = match arg_type(ctx.args, "param") {
        Some(kt) => kt.clone(),
        None => {
            let error = match arg_held(ctx.args, "param") {
                Some(Held::Object(object)) => KError::new(KErrorKind::TypeMismatch {
                    arg: "param".to_string(),
                    expected: "ProperType".to_string(),
                    got: object.ktype().name(),
                }),
                _ => KError::new(KErrorKind::MissingArg("param".to_string())),
            };
            return Action::Done(Err(error));
        }
    };
    let param = param_kt.name();
    // Abstract higher-kinded SIG slot — not a constructible union, so the variant schema
    // is empty (identity ignores it anyway).
    let member = NominalMember::pending(
        "_typeconstructor".into(),
        ScopeId::SENTINEL,
        KKind::TypeConstructor,
    );
    member.fill(NominalSchema::TypeConstructor {
        schema: std::collections::HashMap::new(),
        param_names: vec![param],
    });
    let set = Rc::new(RecursiveSet::new(vec![member]));
    let kt = ctx
        .scope
        .region
        .alloc_ktype(KType::SetRef { set, index: 0 });
    Action::DoneWitnessed(ctx.scope.seal_value(Carried::Type(kt), None))
}

#[cfg(test)]
mod tests {
    use crate::builtins::test_support::{parse_one, run, run_one_type, run_root_silent};
    use crate::machine::core::FrameStorage;
    use crate::machine::execute::KoanRuntime;
    use crate::machine::model::types::{KKind, ProjectedSchema, RecursiveSet};
    use crate::machine::model::{KObject, KType};
    use crate::machine::{BindingIndex, ScopeId};

    /// Assert `kt` is a `TypeConstructor`-kind `SetRef` whose projected `param_names` equal
    /// `expected`; returns the member's name.
    fn assert_type_constructor(kt: &KType<'_>, expected: &[&str]) -> String {
        match kt {
            KType::SetRef { set, index } if set.member(*index).kind == KKind::TypeConstructor => {
                match RecursiveSet::projected_schema(set, *index) {
                    ProjectedSchema::TypeConstructor { param_names, .. } => {
                        let want: Vec<String> = expected.iter().map(|s| s.to_string()).collect();
                        assert_eq!(param_names, want);
                    }
                    _ => {
                        panic!("TypeConstructor-kind member must project a TypeConstructor schema")
                    }
                }
                set.member(*index).name.clone()
            }
            other => panic!("expected a TypeConstructor SetRef, got {other:?}"),
        }
    }

    /// Pins the template shape the builtin returns before opaque ascription re-mints it.
    #[test]
    fn type_constructor_builtin_returns_ktype_value() {
        let region = FrameStorage::run_root();
        let scope = run_root_silent(&region);
        let result = run_one_type(scope, parse_one("TEMPLATE Type"));
        match result {
            kt @ KType::SetRef { set, index } => {
                let name = assert_type_constructor(kt, &["Type"]);
                assert_eq!(set.member(*index).scope_id, ScopeId::SENTINEL);
                assert_eq!(name, "_typeconstructor");
            }
            other => panic!("expected SetRef type, got {other:?}"),
        }
    }

    /// Pins the LET-routing + `register_type` path for a higher-kinded SIG slot.
    #[test]
    fn sig_declares_higher_kinded_slot() {
        let region = FrameStorage::run_root();
        let scope = run_root_silent(&region);
        run(scope, "SIG Monad = ((LET Wrap = (TEMPLATE Type)))");
        let s = match scope.resolve_type("Monad") {
            Some(KType::Signature { sig, .. }) => *sig,
            _ => panic!("Monad must bind a Signature KType"),
        };
        let wrap_kt: &KType = s.decl_scope().bindings().expect_type("Wrap");
        assert_type_constructor(wrap_kt, &["Type"]);
    }

    /// Pins the dispatch path for an FN return type `:(Number AS Wrap)` against a
    /// root-scope-bound TypeConstructor — the `AS` keyworded builtin lowers it to a
    /// `ConstructorApply` carrier.
    #[test]
    fn fn_return_type_constructor_apply_root_scope() {
        let region = FrameStorage::run_root();
        let scope = run_root_silent(&region);
        scope.register_type(
            "Wrap".into(),
            wrap_type_constructor(ScopeId::from_raw(0, 0xC0DE)),
            BindingIndex::BUILTIN,
        );
        let mut sched = KoanRuntime::new();
        let id = sched.dispatch_in_scope(
            parse_one("LET pure = (FN (PURE a :Number) -> :(Number AS Wrap) = (1))"),
            scope,
        );
        sched.execute().expect("scheduler should run");
        match sched.result_error(id) {
            Ok(()) => {}
            Err(e) => panic!("FN with :(Number AS Wrap) return failed: {}", e),
        }
        let pure = scope.bindings().expect_value("pure");
        let f = match pure {
            KObject::KFunction(f) => *f,
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
        let region = FrameStorage::run_root();
        let scope = run_root_silent(&region);
        let src = "SIG Monad = ((LET Wrap = (TEMPLATE Type)) \
             (VAL pure :(FN (x :Number) -> :(Number AS Wrap))))";
        let exprs = parse(src).expect("parse should succeed");
        let mut sched = KoanRuntime::new();
        let mut ids = Vec::new();
        for expr in exprs {
            ids.push(sched.dispatch_in_scope(expr, scope));
        }
        match sched.execute() {
            Ok(()) => {}
            Err(e) => panic!("scheduler errored: {}", e),
        }
        for (i, id) in ids.iter().enumerate() {
            if let Err(e) = sched.result_error(*id) {
                panic!("expr {} errored: {}", i, e);
            }
        }
        let s = match scope.resolve_type("Monad") {
            Some(KType::Signature { sig, .. }) => *sig,
            other => panic!("Monad must bind a Signature KType, got {:?}", other),
        };
        let wrap_kt: &KType = s.decl_scope().bindings().expect_type("Wrap");
        assert_type_constructor(wrap_kt, &["Type"]);
        // A SIG-body `VAL pure :T` slot lives in `bindings.types` under its value-class
        // name, carrying the declared type directly.
        let kt: &KType = s.decl_scope().bindings().expect_type("pure");
        match kt {
            KType::KFunction { params, ret } => {
                assert_eq!(params.get("x"), Some(&KType::Number));
                assert_eq!(params.len(), 1);
                match ret.as_ref() {
                    KType::ConstructorApply { ctor, args } => {
                        assert_type_constructor(ctor.as_ref(), &["Type"]);
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
        let region = FrameStorage::run_root();
        let scope = run_root_silent(&region);
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
            Some(kt) => {
                let name = assert_type_constructor(&kt, &["Type"]);
                assert_eq!(name, "Wrap");
            }
            other => panic!(
                "expected TypeConstructor in type_members[Wrap], got {:?}",
                other,
            ),
        }
    }

    /// A root-scope-bound `Wrap` TypeConstructor `SetRef` with the given origin scope id.
    fn wrap_type_constructor<'a>(scope_id: ScopeId) -> KType<'a> {
        let set = RecursiveSet::singleton(
            "Wrap".into(),
            scope_id,
            crate::machine::model::types::NominalSchema::TypeConstructor {
                schema: std::collections::HashMap::new(),
                param_names: vec!["Type".into()],
            },
        );
        KType::SetRef { set, index: 0 }
    }
}
