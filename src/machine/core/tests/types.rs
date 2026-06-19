//! `register_type` / `resolve_type` tests: type bindings land in `types` (not `data`),
//! `resolve_type` walks the outer chain, and inner scopes shadow outer type bindings.

use super::super::{KoanRegion, Scope};
use crate::builtins::test_support::run_root_bare;
use crate::machine::model::types::KType;
use crate::machine::BindingIndex;

#[test]
fn register_type_inserts_into_types_map_not_data() {
    let region = KoanRegion::new();
    let scope = run_root_bare(&region);
    scope.register_type("Foo".into(), KType::Number, BindingIndex::BUILTIN);
    assert!(scope.bindings().types().get("Foo").is_some());
    assert!(
        scope.bindings().data().get("Foo").is_none(),
        "type binding must not appear in data map",
    );
}

#[test]
fn resolve_type_walks_outer_chain_and_returns_none_past_root() {
    let region = KoanRegion::new();
    let root = run_root_bare(&region);
    root.register_type("Foo".into(), KType::Number, BindingIndex::BUILTIN);
    let child = region.alloc_scope(Scope::child_under(root));
    assert!(matches!(child.resolve_type("Foo"), Some(KType::Number)));
    assert!(
        child.resolve_type("Nope").is_none(),
        "unbound name past run-root yields None, not panic",
    );
}

#[test]
fn resolve_type_inner_scope_shadows_outer() {
    let region = KoanRegion::new();
    let root = run_root_bare(&region);
    // User (non-BUILTIN) types: a builtin is unshadowable and would resolve root-first,
    // so this exercises the user-vs-user innermost-wins walk.
    root.register_type("Foo".into(), KType::Number, BindingIndex::value(1));
    let child = region.alloc_scope(Scope::child_under(root));
    child.register_type("Foo".into(), KType::Str, BindingIndex::value(1));
    assert!(matches!(child.resolve_type("Foo"), Some(KType::Str)));
    assert!(matches!(root.resolve_type("Foo"), Some(KType::Number)));
}
