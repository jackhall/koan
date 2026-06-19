use super::*;
use crate::builtins::test_support::run_root_silent;
use crate::machine::model::ast::TypeIdentifier;
use crate::machine::BindingIndex;
use crate::machine::RuntimeArena;

fn leaf(n: &str) -> TypeIdentifier {
    TypeIdentifier::leaf(n.into())
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
            arena.alloc_object(KObject::Number(7.0)),
            BindingIndex::BUILTIN,
        )
        .expect("bind_value");
    let mut el = Elaborator::new(scope);
    match elaborate_type_identifier(&mut el, &leaf("Gee")) {
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
    match elaborate_type_identifier(&mut el, &leaf("NopeType")) {
        ElabResult::Unbound(msg) => assert!(
            msg.contains("unknown type name") && msg.contains("NopeType"),
            "expected an unknown-type-name message naming `NopeType`, got: {msg}",
        ),
        other => panic!("expected Unbound, got {:?}", other),
    }
}

/// A bare leaf naming a member of the enclosing `RECURSIVE TYPES` group lowers to a
/// transient `RecursiveRef` back-edge — the block's threading, independent of source order.
/// A non-member falls through to ordinary resolution.
#[test]
fn recursive_group_member_lowers_to_recursive_ref() {
    let arena = RuntimeArena::new();
    let parent = run_root_silent(&arena);
    let set = std::rc::Rc::new(RecursiveSet::new(vec![
        NominalMember::pending("A".into(), parent.id, KKind::NewType),
        NominalMember::pending("B".into(), parent.id, KKind::NewType),
    ]));
    let child = arena.alloc_scope(Scope::child_recursive_group(parent, set));
    let mut el = Elaborator::new(child);
    match elaborate_type_identifier(&mut el, &leaf("B")) {
        ElabResult::Done(KType::RecursiveRef(name)) => assert_eq!(name, "B"),
        other => panic!("expected a RecursiveRef back-edge for a group member, got {other:?}"),
    }
    let mut el2 = Elaborator::new(child);
    assert!(
        matches!(
            elaborate_type_identifier(&mut el2, &leaf("Nope")),
            ElabResult::Unbound(_)
        ),
        "a non-member must fall through to ordinary resolution",
    );
}

#[test]
fn constructor_apply_name_renders_surface_form() {
    use crate::machine::model::types::NominalSchema;
    let member = NominalMember::pending(
        "Wrap".into(),
        ScopeId::from_raw(0, 0xC0DE),
        KKind::TypeConstructor,
    );
    member.fill(NominalSchema::TypeConstructor {
        schema: std::collections::HashMap::new(),
        param_names: vec!["Type".into()],
    });
    let set = std::rc::Rc::new(RecursiveSet::new(vec![member]));
    let ctor = KType::SetRef { set, index: 0 };
    let app = KType::ConstructorApply {
        ctor: Box::new(ctor),
        args: vec![KType::Number],
    };
    assert_eq!(app.name(), ":(Wrap Number)");
}
