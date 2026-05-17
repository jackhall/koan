//! `register_type` rewire + `resolve_type` tests.
//!
//! Pin the three load-bearing properties of the rewire:
//! - storage flip: `register_type` writes `types`, not `data`;
//! - `resolve_type` outer-chain walk;
//! - inner-scope shadowing of outer type bindings.

use super::super::{RuntimeArena, Scope};
use crate::builtins::test_support::run_root_bare;
use crate::machine::model::types::KType;


#[test]
fn register_type_inserts_into_types_map_not_data() {
    let arena = RuntimeArena::new();
    let scope = run_root_bare(&arena);
    scope.register_type("Foo".into(), KType::Number);
    assert!(scope.bindings().types().get("Foo").is_some());
    assert!(
        scope.bindings().data().get("Foo").is_none(),
        "post-1.4: type binding must not appear in data map",
    );
}

#[test]
fn resolve_type_walks_outer_chain_and_returns_none_past_root() {
    let arena = RuntimeArena::new();
    let root = run_root_bare(&arena);
    root.register_type("Foo".into(), KType::Number);
    let child = arena.alloc_scope(Scope::child_under(root));
    assert!(matches!(child.resolve_type("Foo"), Some(KType::Number)));
    assert!(
        child.resolve_type("Nope").is_none(),
        "unbound name past run-root yields None, not panic",
    );
}

#[test]
fn resolve_type_inner_scope_shadows_outer() {
    let arena = RuntimeArena::new();
    let root = run_root_bare(&arena);
    root.register_type("Foo".into(), KType::Number);
    let child = arena.alloc_scope(Scope::child_under(root));
    child.register_type("Foo".into(), KType::Str);
    assert!(matches!(child.resolve_type("Foo"), Some(KType::Str)));
    assert!(matches!(root.resolve_type("Foo"), Some(KType::Number)));
}
