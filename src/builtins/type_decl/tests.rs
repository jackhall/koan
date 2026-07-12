use crate::builtins::test_support::{parse_one, run, run_root_silent};
use crate::machine::core::run_root_storage;
use crate::machine::model::ast::ExpressionPart;
use crate::machine::model::types::{AbstractSource, KKind, KType, ProjectedSchema, RecursiveSet};

/// Resolve a SIG-declared member's stored `KType` out of the signature's decl-scope type table.
fn member_type<'a>(
    scope: &'a crate::machine::Scope<'a>,
    sig_name: &str,
    member: &str,
) -> KType<'a> {
    let sig = match scope.resolve_type(sig_name) {
        Some(KType::Signature { sig, .. }) => *sig,
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
        Some(KType::Signature { sig, .. }) => *sig,
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
    use crate::machine::ScopeId;
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
