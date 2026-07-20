use super::*;
use crate::builtins::test_support::TestRun;
use crate::machine::core::StoredReach;
use crate::machine::core::{run_root_storage, FrameStorageExt};
use crate::machine::model::ast::TypeIdentifier;
use crate::machine::model::Record;
use crate::machine::BindingIndex;

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
    let test_run = TestRun::silent(&region);
    let scope = test_run.scope;
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
    let types = test_run.types.clone();
    let mut el = Elaborator::new(scope);
    match elaborate_type_identifier(&mut el, &leaf("Gee"), &types) {
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
    let test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    let types = test_run.types.clone();
    let mut el = Elaborator::new(scope);
    match elaborate_type_identifier(&mut el, &leaf("NopeType"), &types) {
        TypeResolution::Unbound(msg) => assert!(
            msg.contains("unknown type name") && msg.contains("NopeType"),
            "expected an unknown-type-name message naming `NopeType`, got: {msg}",
        ),
        other => panic!("expected Unbound, got {:?}", other),
    }
}

/// A bare leaf naming a member of the enclosing `RECURSIVE TYPES` window lowers to that member's
/// relative sibling handle — the block's cross-order resolution, independent of source order. A
/// non-member falls through to ordinary resolution.
#[test]
fn recursive_group_member_lowers_to_sibling() {
    let region = run_root_storage();
    let parent_test_run = TestRun::silent(&region);
    let parent = parent_test_run.scope;
    let window = RecursiveGroupWindow::new(
        vec![("A".into(), KKind::NewType), ("B".into(), KKind::NewType)],
        None,
    );
    let child = region
        .brand()
        .alloc_scope(Scope::child_recursive_group(parent, window));
    let types = parent_test_run.types.clone();
    let mut el = Elaborator::new(child);
    match elaborate_type_identifier(&mut el, &leaf("B"), &types) {
        TypeResolution::Done(kt) => assert_eq!(kt, types.intern(TypeNode::Sibling(1))),
        other => panic!("expected a sibling back-edge for a window member, got {other:?}"),
    }
    let mut el2 = Elaborator::new(child);
    assert!(
        matches!(
            elaborate_type_identifier(&mut el2, &leaf("Nope"), &types),
            TypeResolution::Unbound(_)
        ),
        "a non-member must fall through to ordinary resolution",
    );
}

/// A `UNION`'s own binder names no single variant: it resolves to the union of every announced
/// member, which is what a variant payload referencing the union's own name means.
#[test]
fn window_binder_resolves_to_the_union_of_its_members() {
    let region = run_root_storage();
    let test_run = TestRun::silent(&region);
    let types = test_run.types.clone();
    let window = RecursiveGroupWindow::new(
        vec![
            ("Leaf".into(), KKind::NewType),
            ("Node".into(), KKind::NewType),
        ],
        Some("Tree".into()),
    );
    let mut el = Elaborator::new(test_run.scope).with_window(window.clone());
    match elaborate_type_identifier(&mut el, &leaf("Tree"), &types) {
        TypeResolution::Done(kt) => assert_eq!(kt, window.binder_union(&types)),
        other => panic!("expected the binder union, got {other:?}"),
    }
}

/// A member of a multi-member window has no identity until the whole window seals: identity is
/// computed over the group's entire reference structure, so an intermediate fill defers. The last
/// fill seals, and only then does the member's handle install.
#[test]
fn block_member_defers_until_the_window_seals() {
    let region = run_root_storage();
    let test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    let types = test_run.types.clone();
    let window = RecursiveGroupWindow::new(
        vec![
            ("Node".into(), KKind::NewType),
            ("Leaf".into(), KKind::NewType),
        ],
        None,
    );
    let fill = |name: &str, repr: KType, index: BindingIndex| {
        finalize_nominal_member(
            scope,
            &window,
            name,
            |_| RelativeSchema::NewType(repr),
            index,
            &types,
        )
    };
    match fill("Node", KType::NUMBER, BindingIndex::value(2)) {
        SealOutcome::Deferred => {}
        other => panic!(
            "the first of two members must defer, got {}",
            outcome_tag(&other)
        ),
    }
    let sealed = match fill("Leaf", KType::STR, BindingIndex::value(3)) {
        SealOutcome::Sealed(kt) => *kt,
        other => panic!("the last fill must seal, got {}", outcome_tag(&other)),
    };
    assert_eq!(
        sealed,
        window.sealed().expect("sealed").members[1],
        "the outcome is Leaf's own member handle",
    );

    // A different statement declaring `Leaf` over different content is a redeclaration: the
    // upsert collides with the identity this window installed.
    let other_window = RecursiveGroupWindow::new(vec![("Leaf".into(), KKind::NewType)], None);
    match finalize_nominal_member(
        scope,
        &other_window,
        "Leaf",
        |_| RelativeSchema::NewType(KType::BOOL),
        BindingIndex::value(4),
        &types,
    ) {
        SealOutcome::Rebind(e) => assert!(
            matches!(&e.kind, crate::machine::KErrorKind::Rebind { name } if name == "Leaf"),
            "expected Rebind naming Leaf, got {e}",
        ),
        other => panic!(
            "expected Rebind on redeclaration, got {}",
            outcome_tag(&other)
        ),
    }
}

fn outcome_tag(outcome: &SealOutcome<'_>) -> &'static str {
    match outcome {
        SealOutcome::Sealed(_) => "Sealed",
        SealOutcome::Deferred => "Deferred",
        SealOutcome::DanglingRef(_) => "DanglingRef",
        SealOutcome::Rebind(_) => "Rebind",
    }
}

#[test]
fn constructor_apply_name_renders_surface_form() {
    let types = TypeRegistry::new();
    let ctor = RecursiveGroupWindow::seal_singleton(
        "Wrap".into(),
        RelativeSchema::TypeConstructor {
            schema: std::collections::HashMap::new(),
            param_names: vec!["Type".into()],
        },
        None,
        &types,
    );
    let app = types.constructor_apply(
        ctor,
        Record::from_pairs([("Type".to_string(), KType::NUMBER)]),
    );
    assert_eq!(app.name(&types), ":(Wrap {Type = Number})");
}
