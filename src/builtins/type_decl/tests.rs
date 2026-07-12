use crate::builtins::test_support::{parse_one, run, run_root_silent};
use crate::machine::core::run_root_storage;
use crate::machine::execute::KoanRuntime;
use crate::machine::model::ast::ExpressionPart;
use crate::machine::model::types::{
    AbstractSource, KKind, KType, NominalSchema, ProjectedSchema, RecursiveSet, SigSource,
};
use crate::machine::model::KObject;
use crate::machine::{BindingIndex, ScopeId};

/// Resolve a SIG-declared member's stored `KType` out of the signature's decl-scope type table.
fn member_type<'a>(
    scope: &'a crate::machine::Scope<'a>,
    sig_name: &str,
    member: &str,
) -> KType<'a> {
    let sig = match scope.resolve_type(sig_name) {
        Some(KType::Signature {
            sig: SigSource::Declared(sig),
            ..
        }) => *sig,
        other => panic!("{sig_name} must bind a Signature, got {other:?}"),
    };
    sig.decl_scope()
        .bindings()
        .lookup_type(member, None)
        .and_then(crate::machine::NameLookup::bound)
        .cloned()
        .unwrap_or_else(|| panic!("member `{member}` must live in {sig_name}'s type table"))
}

/// `TYPE Elt` binds `AbstractType { source: Sig(decl_scope), name: "Elt" }`.
#[test]
fn bare_type_binds_abstract_member() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(scope, "SIG Container = ((TYPE Elt))");
    let sig = match scope.resolve_type("Container") {
        Some(KType::Signature {
            sig: SigSource::Declared(sig),
            ..
        }) => *sig,
        other => panic!("Container must bind a Signature, got {other:?}"),
    };
    match member_type(scope, "Container", "Elt") {
        KType::AbstractType {
            source: AbstractSource::Sig(id),
            name,
        } => {
            assert_eq!(name, "Elt");
            assert_eq!(id, sig.decl_scope().id);
        }
        other => panic!("Elt must be an abstract Sig-sourced member, got {other:?}"),
    }
}

/// `TYPE (Type AS Wrap)` binds a sentinel `TypeConstructor` `SetRef` named `Wrap` with
/// `param_names == ["Type"]`.
#[test]
fn hk_type_binds_sentinel_constructor() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(scope, "SIG Monad = ((TYPE (Type AS Wrap)))");
    match member_type(scope, "Monad", "Wrap") {
        KType::SetRef { set, index } if set.member(index).kind == KKind::TypeConstructor => {
            assert_eq!(set.member(index).scope_id, ScopeId::SENTINEL);
            assert_eq!(set.member(index).name, "Wrap");
            match RecursiveSet::projected_schema(&set, index) {
                ProjectedSchema::TypeConstructor { param_names, .. } => {
                    assert_eq!(param_names, vec!["Type".to_string()]);
                }
                _ => panic!("expected a TypeConstructor schema"),
            }
        }
        other => panic!("Wrap must be a sentinel TypeConstructor SetRef, got {other:?}"),
    }
}

/// `TYPE Elt` outside a SIG body errors.
#[test]
fn bare_type_outside_sig_errors() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(scope, "TYPE Elt");
    assert!(
        scope.resolve_type("Elt").is_none(),
        "TYPE outside a SIG body must not bind",
    );
}

/// `TYPE (Key Val AS Dict)` — two parameters before `AS` — hits the arity-above-1 error.
#[test]
fn hk_arity_above_one_errors() {
    let expr = parse_one("TYPE (Key Val AS Dict)");
    let inner = match &expr.parts.get(1).expect("TYPE decl part").value {
        ExpressionPart::Expression(inner) => inner.as_ref().clone(),
        other => panic!("expected a parenthesized decl, got {other:?}"),
    };
    let error = super::parse_hk_decl(&inner).expect_err("arity above 1 must error");
    assert!(
        error.to_string().contains("arity above 1"),
        "expected the arity message, got {error}",
    );
}

/// A `VAL item :Elt` slot after `TYPE Elt` records the abstract member as its declared type.
#[test]
fn val_slot_after_type_records_abstract_member() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(scope, "SIG Container = ((TYPE Elt) (VAL item :Elt))");
    match member_type(scope, "Container", "item") {
        KType::AbstractType {
            source: AbstractSource::Sig(_),
            name,
        } => assert_eq!(name, "Elt"),
        other => panic!("item's declared type must be the abstract Elt, got {other:?}"),
    }
}

/// End-to-end: a module ascribed to a SIG with a `TYPE Elt` member mints a per-call
/// `AbstractType { source: Module(view) }` for `Elt` in the view's `type_members`.
#[test]
fn opaque_ascription_mints_module_abstract_for_type_member() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "MODULE Impl = ((LET Elt = Number) (LET item = 0))\n\
         SIG Container = ((TYPE Elt) (VAL item :Number))\n\
         LET View = (Impl :| Container)",
    );
    let view = match scope.resolve_type("View") {
        Some(KType::Module { module }) => *module,
        other => panic!("View must be a module identity, got {other:?}"),
    };
    let elt = view.type_members.borrow().get("Elt").cloned();
    match elt {
        Some(KType::AbstractType {
            source: AbstractSource::Module(_),
            name,
        }) => assert_eq!(name, "Elt"),
        other => panic!("Elt must mint a Module-sourced abstract type, got {other:?}"),
    }
}

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
                _ => panic!("TypeConstructor-kind member must project a TypeConstructor schema"),
            }
            set.member(*index).name.clone()
        }
        other => panic!("expected a TypeConstructor SetRef, got {other:?}"),
    }
}

/// A root-scope-bound `Wrap` TypeConstructor `SetRef` with the given origin scope id.
fn wrap_type_constructor<'a>(scope_id: ScopeId) -> KType<'a> {
    let set = RecursiveSet::singleton(
        "Wrap".into(),
        scope_id,
        NominalSchema::TypeConstructor {
            schema: std::collections::HashMap::new(),
            param_names: vec!["Type".into()],
        },
    );
    KType::SetRef { set, index: 0 }
}

/// Pins the dispatch path for an FN return type `:(Number AS Wrap)` against a
/// root-scope-bound TypeConstructor — the `AS` keyworded builtin lowers it to a
/// `ConstructorApply` carrier.
#[test]
fn fn_return_type_constructor_apply_root_scope() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    scope.register_builtin_type(
        "Wrap".into(),
        wrap_type_constructor(ScopeId::from_raw(0, 0xC0DE)),
        BindingIndex::BUILTIN,
    );
    let mut runtime = KoanRuntime::new();
    let id = runtime.dispatch_in_scope(
        parse_one("LET pure = (FN (PURE a :Number) -> :(Number AS Wrap) = (1))"),
        scope,
    );
    runtime.execute().expect("scheduler should run");
    match runtime.result_error(id) {
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

/// End-to-end smoke for a monad-shaped signature: `TYPE (Type AS Wrap)` precedes
/// `VAL pure` so the inner `:(Number AS Wrap)` resolves synchronously against the
/// SIG decl-scope's `bindings.types["Wrap"]` entry.
#[test]
fn monad_signature_smoke() {
    use crate::parse::parse;
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    let src = "SIG Monad = ((TYPE (Type AS Wrap)) \
         (VAL pure :(FN (x :Number) -> :(Number AS Wrap))))";
    let exprs = parse(src).expect("parse should succeed");
    let mut runtime = KoanRuntime::new();
    let mut ids = Vec::new();
    for expr in exprs {
        ids.push(runtime.dispatch_in_scope(expr, scope));
    }
    match runtime.execute() {
        Ok(()) => {}
        Err(e) => panic!("scheduler errored: {}", e),
    }
    for (i, id) in ids.iter().enumerate() {
        if let Err(e) = runtime.result_error(*id) {
            panic!("expr {} errored: {}", i, e);
        }
    }
    let s = match scope.resolve_type("Monad") {
        Some(KType::Signature {
            sig: SigSource::Declared(sig),
            ..
        }) => *sig,
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

/// `(M.Wrap)` after opaque ascription resolves through the module's `type_members` to the
/// per-call-minted constructor variant. The module supplies the higher-kinded abstract `Wrap`
/// slot with a real arity-1 constructor (`LET Wrap = Wrapper`) — a proper type would fail the
/// slot's kind/arity check.
#[test]
fn module_attr_access_returns_type_constructor() {
    use crate::builtins::test_support::register_arity1_constructor;
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    register_arity1_constructor(scope, "Wrapper");
    run(
        scope,
        "SIG MonadSig = ((TYPE (Type AS Wrap)))\n\
         MODULE IntList = ((LET Wrap = Wrapper))\n\
         LET Mo = (IntList :| MonadSig)",
    );
    let mo = match scope.resolve_type("Mo") {
        Some(KType::Module { module }) => *module,
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
            other
        ),
    }
}
