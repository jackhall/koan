use crate::builtins::test_support::{lookup_module, parse_one, TestRun};
use crate::machine::model::ExpressionPart;
use crate::machine::model::KObject;
use crate::machine::model::Record;
use crate::machine::model::TypeRegistry;
use crate::machine::model::{
    constructor_param_names, declarator_window, KKind, KType, RelativeSchema, TypeNode,
};
use crate::machine::run_root_storage;
use crate::machine::{BindingIndex, ScopeId};

/// Resolve a SIG-declared type member's stored `KType` out of the signature's schema —
/// abstract members (`TYPE`) and manifest members (`LET`) both live there, classified by
/// representation at SIG finish.
fn member_type(
    scope: &crate::machine::Scope<'_>,
    types: &TypeRegistry,
    sig_name: &str,
    member: &str,
) -> KType {
    let handle = scope
        .resolve_type(sig_name)
        .copied()
        .unwrap_or_else(|| panic!("{sig_name} must bind a type"));
    let schema = match types.node(handle) {
        TypeNode::Signature { schema, .. } => schema,
        _ => panic!("{sig_name} must bind a Signature, got {handle:?}"),
    };
    if let Some(kt) = schema.abstract_members.get(member) {
        return *kt;
    }
    schema
        .manifest_members
        .get(member)
        .copied()
        .unwrap_or_else(|| panic!("member `{member}` must live in {sig_name}'s type table"))
}

/// `TYPE Elt` binds `AbstractType { source: SENTINEL, name: "Elt" }` — a SIG-declared abstract
/// member's binder is the canonical sentinel (ruling 12), never a per-declaration id.
#[test]
fn bare_type_binds_abstract_member() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    test_run.run("SIG Container = ((TYPE Elt))");
    match test_run
        .types()
        .node(member_type(scope, test_run.types(), "Container", "Elt"))
    {
        TypeNode::AbstractType { source, name, .. } => {
            assert_eq!(name, "Elt");
            assert_eq!(source, ScopeId::SENTINEL);
        }
        _ => panic!("Elt must be an abstract member sourced at the canonical binder"),
    }
}

/// `TYPE (Type AS Wrap)` binds an `AbstractType` named `Wrap`, sourced at the canonical binder,
/// carrying `param_names == ["Type"]`.
#[test]
fn hk_type_binds_abstract_constructor() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    test_run.run("SIG Monad = ((TYPE (Type AS Wrap)))");
    match test_run
        .types()
        .node(member_type(scope, test_run.types(), "Monad", "Wrap"))
    {
        TypeNode::AbstractType {
            source,
            name,
            param_names,
            ..
        } => {
            assert_eq!(name, "Wrap");
            assert_eq!(source, ScopeId::SENTINEL);
            assert_eq!(param_names, vec!["Type".to_string()]);
        }
        _ => panic!("Wrap must be an abstract constructor member"),
    }
}

/// An abstract constructor member classifies as `KKind::TypeConstructor`; its first-order
/// sibling as `KKind::ProperType`.
#[test]
fn abstract_member_kind_tracks_parameters() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    test_run.run("SIG Monad = ((TYPE Elt) (TYPE (Type AS Wrap)))");
    let types = test_run.types.clone();
    assert_eq!(
        member_type(scope, &types, "Monad", "Wrap").kind_of(&types),
        KKind::TypeConstructor,
    );
    assert_eq!(
        member_type(scope, &types, "Monad", "Elt").kind_of(&types),
        KKind::ProperType,
    );
}

/// `TYPE Elt` outside a SIG body errors.
#[test]
fn bare_type_outside_sig_errors() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    test_run.run("TYPE Elt");
    assert!(
        scope.resolve_type("Elt").is_none(),
        "TYPE outside a SIG body must not bind",
    );
}

/// `TYPE (Key Val AS Dict)` — two parameters before `AS` — declares an arity-2 constructor.
#[test]
fn hk_arity_above_one_declares() {
    let inner = hk_decl_body("TYPE (Key Val AS Dict)");
    let (param_names, member_name) =
        super::parse_hk_decl(&inner).expect("arity above 1 must declare");
    assert_eq!(param_names, vec!["Key".to_string(), "Val".to_string()]);
    assert_eq!(member_name, "Dict");
}

/// A parameter name repeated in one declaration is a shape error — the names key the
/// application record, so they must be distinct.
#[test]
fn hk_duplicate_parameter_name_errors() {
    let inner = hk_decl_body("TYPE (Key Key AS Dict)");
    let error = super::parse_hk_decl(&inner).expect_err("a duplicate parameter name must error");
    assert!(
        error.to_string().contains("duplicate parameter name `Key`"),
        "expected the duplicate-name message, got {error}",
    );
}

/// The parenthesized `(Param... AS Name)` group inside a parsed `TYPE` declaration.
fn hk_decl_body(source: &str) -> crate::machine::model::KExpression<'static> {
    let expr = parse_one(source);
    match &expr.parts.get(1).expect("TYPE decl part").value {
        ExpressionPart::Expression(inner) => inner.as_ref().clone(),
        other => panic!("expected a parenthesized decl, got {other:?}"),
    }
}

/// A `VAL item :Elt` slot after `TYPE Elt` records the abstract member as its declared type. The
/// slot lives in the signature's stored schema (`value_slots`), not the decl scope's type table.
#[test]
fn val_slot_after_type_records_abstract_member() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    test_run.run("SIG Container = ((TYPE Elt) (VAL item :Elt))");
    let handle = scope
        .resolve_type("Container")
        .copied()
        .expect("Container must bind a type");
    let item = match test_run.types().node(handle) {
        TypeNode::Signature { schema, .. } => schema
            .value_slots
            .get("item")
            .copied()
            .expect("item must live in Container's stored schema value_slots"),
        _ => panic!("Container must bind a Signature, got {handle:?}"),
    };
    match test_run.types().node(item) {
        TypeNode::AbstractType { source, name, .. } => {
            assert_eq!(name, "Elt");
            assert_eq!(source, ScopeId::SENTINEL);
        }
        _ => panic!("item's declared type must be the abstract Elt, got {item:?}"),
    }
}

/// End-to-end: a module ascribed to a SIG with a `TYPE Elt` member mints a per-call
/// `AbstractType` for `Elt` in its `type_members`, nonced on the view module's own `ScopeId`. The
/// `source` binder stays the canonical sentinel — generativity rides `nonce` alone — so the mint is
/// a distinct identity from the SIG-decl-time member it was threaded from.
#[test]
fn opaque_ascription_mints_module_abstract_for_type_member() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    test_run.run(
        "MODULE implementation = ((LET Elt = Number) (LET item = 0))\n\
         SIG Container = ((TYPE Elt) (VAL item :Number))\n\
         LET view = (implementation :| Container)",
    );
    let view = lookup_module(scope, "view", &test_run.types);
    let elt = view.type_members.borrow().get("Elt").copied();
    let declared = member_type(scope, test_run.types(), "Container", "Elt");
    match elt {
        Some(minted) => {
            match test_run.types().node(minted) {
                TypeNode::AbstractType {
                    source,
                    name,
                    nonce,
                    ..
                } => {
                    assert_eq!(name, "Elt");
                    assert_eq!(source, ScopeId::SENTINEL);
                    assert_eq!(nonce, Some(view.scope_id()));
                }
                _ => panic!(
                    "Elt must mint an abstract type keyed on the view module, got {minted:?}"
                ),
            }
            assert_ne!(minted, declared, "the mint is not the declaration");
        }
        None => panic!("Elt must mint an abstract type keyed on the view module"),
    }
}

/// Two `:|` applications of one SIG mint distinct opaque slot types: each ascription allocates a
/// fresh child scope, so the per-application `nonce` differs even though `source` and name agree.
#[test]
fn two_ascriptions_of_one_sig_mint_distinct_slot_types() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    test_run.run(
        "MODULE implementation = ((LET Elt = Number) (LET item = 0))\n\
         SIG Container = ((TYPE Elt) (VAL item :Number))\n\
         LET one = (implementation :| Container)\n\
         LET two = (implementation :| Container)",
    );
    let elt = |view_name: &str| {
        lookup_module(scope, view_name, &test_run.types)
            .type_members
            .borrow()
            .get("Elt")
            .copied()
            .expect("each view mints Elt")
    };
    let (one, two) = (elt("one"), elt("two"));
    assert!(matches!(
        test_run.types().node(one),
        TypeNode::AbstractType { .. }
    ));
    assert_ne!(
        one, two,
        "each `:|` application mints its own opaque Elt identity",
    );
    assert_ne!(one.digest(), two.digest());
}

/// Assert `kt` is a type constructor — a declared family (`SetMember`) or a SIG's abstract
/// constructor slot (`AbstractType`) — whose parameter names equal `expected`; returns its name.
fn assert_type_constructor(kt: KType, expected: &[&str], types: &TypeRegistry) -> String {
    let want: Vec<String> = expected.iter().map(|s| s.to_string()).collect();
    let param_names = constructor_param_names(kt, types)
        .unwrap_or_else(|| panic!("expected a type constructor, got {kt:?}"));
    assert_eq!(param_names, want);
    match types.node(kt) {
        TypeNode::SetMember { name, .. } => name,
        TypeNode::AbstractType { name, .. } => name,
        _ => panic!("expected a type constructor, got {kt:?}"),
    }
}

/// A root-scope-bound `Wrap` TypeConstructor member, sealed through the real declaration window.
fn wrap_type_constructor(scope: &crate::machine::Scope<'_>, types: &TypeRegistry) -> KType {
    let window = declarator_window(scope, "Wrap", KKind::TypeConstructor);
    window
        .fill_member(
            0,
            RelativeSchema::TypeConstructor {
                schema: std::collections::HashMap::new(),
                param_names: vec!["Type".into()],
            },
            types,
        )
        .expect("a singleton window seals on its sole fill")
        .members[0]
}

/// Pins the dispatch path for an FN return type `:(Number AS Wrap)` against a
/// root-scope-bound TypeConstructor — the `AS` keyworded builtin lowers it to a
/// `ConstructorApply` carrier.
#[test]
fn fn_return_type_constructor_apply_root_scope() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    let wrap = wrap_type_constructor(scope, test_run.types());
    scope.register_builtin_type("Wrap".into(), wrap, BindingIndex::BUILTIN);
    let runtime = &mut test_run.runtime;
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
        ReturnType::Resolved(handle) => match test_run.types().node(*handle) {
            TypeNode::ConstructorApply { arguments, .. } => {
                assert_eq!(
                    arguments,
                    Record::from_pairs([("Type".to_string(), KType::NUMBER)]),
                );
            }
            _ => panic!("expected Resolved(ConstructorApply), got {:?}", handle),
        },
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
    let mut test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    let src = "SIG Monad = ((TYPE (Type AS Wrap)) \
         (VAL pure :(FN (x :Number) -> :(Number AS Wrap))))";
    let exprs = parse(src).expect("parse should succeed");
    {
        let runtime = &mut test_run.runtime;
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
    }
    let types = test_run.types();
    let handle = scope
        .resolve_type("Monad")
        .copied()
        .expect("Monad must bind a type");
    let schema = match types.node(handle) {
        TypeNode::Signature { schema, .. } => schema,
        _ => panic!("Monad must bind a Signature KType, got {:?}", handle),
    };
    let wrap_kt = schema
        .abstract_members
        .get("Wrap")
        .copied()
        .expect("Wrap must live in Monad's stored schema abstract_members");
    assert_type_constructor(wrap_kt, &["Type"], types);
    // A SIG-body `VAL pure :T` slot lives in the signature's stored schema (`value_slots`),
    // carrying the declared type directly.
    let pure = schema
        .value_slots
        .get("pure")
        .copied()
        .expect("pure must live in Monad's stored schema value_slots");
    match types.node(pure) {
        TypeNode::KFunction { params, ret } => {
            assert_eq!(params.get("x").copied(), Some(KType::NUMBER));
            assert_eq!(params.len(), 1);
            match types.node(ret) {
                TypeNode::ConstructorApply {
                    constructor,
                    arguments,
                } => {
                    assert_type_constructor(constructor, &["Type"], types);
                    assert_eq!(
                        arguments,
                        Record::from_pairs([("Type".to_string(), KType::NUMBER)]),
                    );
                }
                _ => panic!(
                    "pure return type must be ConstructorApply(Wrap, {{Type = Number}}), got {:?}",
                    ret,
                ),
            }
        }
        _ => panic!("pure must be a Function type, got {:?}", pure),
    }
}

/// `(M.Wrap)` after opaque ascription resolves through the module's `type_members` to the
/// per-call-minted constructor variant. The module supplies the higher-kinded abstract `Wrap`
/// slot with a real arity-1 constructor (`LET Wrap = Wrapper`) — a proper type would fail the
/// slot's kind and parameter-name check.
#[test]
fn module_attr_access_returns_type_constructor() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    test_run.run(
        "NEWTYPE (Type AS Wrapper)\n\
         SIG Monad = ((TYPE (Type AS Wrap)))\n\
         MODULE int_list = ((LET Wrap = Wrapper))\n\
         LET mo = (int_list :| Monad)",
    );
    let mo = lookup_module(scope, "mo", &test_run.types);
    let wrap_t = mo.type_members.borrow().get("Wrap").copied();
    match wrap_t {
        Some(kt) => {
            let name = assert_type_constructor(kt, &["Type"], test_run.types());
            assert_eq!(name, "Wrap");
        }
        other => panic!(
            "expected TypeConstructor in type_members[Wrap], got {:?}",
            other
        ),
    }
}
