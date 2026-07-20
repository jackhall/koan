use super::*;
use crate::builtins::test_support::run_root_silent;
use crate::machine::core::StoredReach;
use crate::machine::core::{run_root_storage, FrameStorageExt};
use crate::machine::model::ast::TypeIdentifier;
use crate::machine::model::Record;
use crate::machine::{BindingIndex, ScopeId};

fn leaf(n: &str) -> TypeIdentifier {
    TypeIdentifier::leaf(n.into())
}

/// A Type token cannot name a value — the binding maps enforce the token-class partition — so a
/// Type-class leaf that names no type is an ordinary unknown-name miss, with no value side to
/// consult. The bind that would set up the old "value-language only" layering is itself rejected.
#[test]
fn type_token_cannot_bind_value_side() {
    use crate::machine::model::values::KObject;
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    let error = scope
        .bind_value(
            "Gee".into(),
            region.brand().alloc_object(KObject::Number(7.0)),
            BindingIndex::BUILTIN,
            StoredReach::for_test(None, false),
        )
        .expect_err("a Type token names a type; it may not bind a value");
    assert!(
        matches!(&error.kind, crate::machine::KErrorKind::ShapeError(msg)
            if msg.contains("`Gee` is a Type token")),
        "expected the token-class partition error, got {error}",
    );
    let mut el = Elaborator::new(scope);
    match elaborate_type_identifier(&mut el, &leaf("Gee")) {
        TypeResolution::Unbound(msg) => assert!(
            msg.contains("Gee"),
            "expected an unknown-name miss naming `Gee`, got: {msg}",
        ),
        other => panic!("expected Unbound, got {:?}", other),
    }
}

#[test]
fn unbound_leaf_names_unknown_type() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    let mut el = Elaborator::new(scope);
    match elaborate_type_identifier(&mut el, &leaf("NopeType")) {
        TypeResolution::Unbound(msg) => assert!(
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
    let region = run_root_storage();
    let parent = run_root_silent(&region);
    let set = std::rc::Rc::new(RecursiveSet::new(vec![
        NominalMember::pending("A".into(), parent.id, KKind::NewType),
        NominalMember::pending("B".into(), parent.id, KKind::NewType),
    ]));
    let child = region
        .brand()
        .alloc_scope(Scope::child_recursive_group(parent, set));
    let mut el = Elaborator::new(child);
    match elaborate_type_identifier(&mut el, &leaf("B")) {
        TypeResolution::Done(KType::RecursiveRef(name)) => assert_eq!(name, "B"),
        other => panic!("expected a RecursiveRef back-edge for a group member, got {other:?}"),
    }
    let mut el2 = Elaborator::new(child);
    assert!(
        matches!(
            elaborate_type_identifier(&mut el2, &leaf("Nope")),
            TypeResolution::Unbound(_)
        ),
        "a non-member must fall through to ordinary resolution",
    );
}

#[test]
fn constructor_apply_name_renders_surface_form() {
    use crate::machine::model::types::NominalSchema;
    let set = RecursiveSet::singleton(
        "Wrap".into(),
        ScopeId::from_raw(0, 0xC0DE),
        NominalSchema::TypeConstructor {
            schema: std::collections::HashMap::new(),
            param_names: vec!["Type".into()],
        },
    );
    let ctor = KType::SetRef { set, index: 0 };
    let app = KType::constructor_apply(
        Box::new(ctor),
        Record::from_pairs([("Type".to_string(), KType::Number)]),
    );
    assert_eq!(app.name(), ":(Wrap {Type = Number})");
}
