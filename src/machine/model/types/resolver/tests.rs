use super::*;
use crate::builtins::test_support::run_root_silent;
use crate::machine::model::ast::TypeExpr;
use crate::machine::BindingIndex;
use crate::machine::RuntimeArena;

fn leaf(n: &str) -> TypeExpr {
    TypeExpr::leaf(n.into())
}

/// A Type-class leaf bound only in the value language reports the layering
/// vocabulary, not an unknown-name miss
/// (see [design/typing/functors.md](../../../../../design/typing/functors.md)).
#[test]
fn value_language_leaf_names_layering() {
    use crate::machine::model::values::KObject;
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    scope
        .bind_value(
            "Gee".into(),
            arena.alloc(KObject::Number(7.0)),
            BindingIndex::BUILTIN,
        )
        .expect("bind_value");
    let mut el = Elaborator::new(scope);
    match elaborate_type_expr(&mut el, &leaf("Gee")) {
        ElabResult::Unbound(msg) => assert!(
            msg.contains("value-language only") && msg.contains("Gee"),
            "expected a value-language layering message naming `Gee`, got: {msg}",
        ),
        other => panic!("expected Unbound, got {:?}", other),
    }
}

#[test]
fn unbound_leaf_names_unknown_type() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    let mut el = Elaborator::new(scope);
    match elaborate_type_expr(&mut el, &leaf("NopeType")) {
        ElabResult::Unbound(msg) => assert!(
            msg.contains("unknown type name") && msg.contains("NopeType"),
            "expected an unknown-type-name message naming `NopeType`, got: {msg}",
        ),
        other => panic!("expected Unbound, got {:?}", other),
    }
}

#[test]
fn constructor_apply_name_renders_surface_form() {
    let ctor = KType::UserType {
        kind: UserTypeKind::TypeConstructor {
            schema: std::rc::Rc::new(std::collections::HashMap::new()),
            param_names: vec!["Type".into()],
        },
        scope_id: ScopeId::from_raw(0, 0xC0DE),
        name: "Wrap".into(),
    };
    let app = KType::ConstructorApply {
        ctor: Box::new(ctor),
        args: vec![KType::Number],
    };
    assert_eq!(app.name(), ":(Wrap Number)");
}
