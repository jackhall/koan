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

/// A bare leaf naming a member of the enclosing `RECURSIVE TYPES` group lowers to a
/// transient `RecursiveRef` back-edge — the block's threading, independent of source order.
/// A non-member falls through to ordinary resolution.
#[test]
fn recursive_group_member_lowers_to_recursive_ref() {
    let region = run_root_storage();
    let parent_test_run = TestRun::silent(&region);
    let parent = parent_test_run.scope;
    let set = std::rc::Rc::new(RecursiveSet::new(vec![
        NominalMember::pending("A".into(), KKind::NewType),
        NominalMember::pending("B".into(), KKind::NewType),
    ]));
    let child = region
        .brand()
        .alloc_scope(Scope::child_recursive_group(parent, set));
    let types = parent_test_run.types.clone();
    let mut el = Elaborator::new(child);
    match elaborate_type_identifier(&mut el, &leaf("B"), &types) {
        TypeResolution::Done(KType::RecursiveRef(name)) => assert_eq!(name, "B"),
        other => panic!("expected a RecursiveRef back-edge for a group member, got {other:?}"),
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

/// A `RECURSIVE TYPES` block pre-installs every member's `SetRef` over one shared set at
/// `BindingIndex::value(0)`, below any statement's own index. `finalize_nominal_member`
/// recovers that pre-install by its *unfilled* member and seals into the shared set rather
/// than minting a singleton; a parallel finalize of the same declaration (equal
/// `BindingIndex`) short-circuits on the installed identity without rebuilding the schema;
/// and a different statement declaring the same name with different content is a
/// redeclaration — `Rebind`.
#[test]
fn block_member_seals_shared_set_then_short_circuits_before_rebind() {
    use crate::machine::model::types::NominalSchema;
    let region = run_root_storage();
    let test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    let set = std::rc::Rc::new(RecursiveSet::new(vec![
        NominalMember::pending("Node".into(), KKind::NewType),
        NominalMember::pending("Leaf".into(), KKind::NewType),
    ]));
    for (index, member) in ["Node", "Leaf"].iter().enumerate() {
        scope.preinstall_identity(
            (*member).to_string(),
            KType::SetRef {
                set: std::rc::Rc::clone(&set),
                index,
            },
            BindingIndex::value(0),
        );
    }
    // Member 0's own declaration finalizes at its statement index: the pre-install's unfilled
    // member is this declaration's contribution, so the schema fills the shared set.
    let sealed = match finalize_nominal_member(
        scope,
        "Node",
        KKind::NewType,
        |_| SchemaSealResult::Ok(NominalSchema::NewType(Box::new(KType::Number))),
        BindingIndex::value(2),
    ) {
        SealOutcome::Sealed(kt) => kt,
        SealOutcome::DanglingRef(missing) => panic!("unexpected dangling ref `{missing}`"),
        SealOutcome::Rebind(e) => panic!("a block member's first seal must not Rebind: {e}"),
    };
    match sealed {
        KType::SetRef {
            set: installed,
            index,
        } => {
            assert!(
                std::rc::Rc::ptr_eq(installed, &set),
                "the seal must fill the block's shared set, not a fresh singleton",
            );
            assert_eq!(*index, 0, "Node is the shared set's member 0");
        }
        other => panic!("expected a SetRef identity, got {other:?}"),
    }
    assert!(
        !set.member(1).is_filled(),
        "sibling member `Leaf` seals at its own declaration, not this one",
    );
    // Parallel finalize of the same declaration: the stored index equals this one, so the
    // installed identity comes straight back — the schema builder never runs.
    let again = match finalize_nominal_member(
        scope,
        "Node",
        KKind::NewType,
        |_| panic!("a parallel finalize must short-circuit before building the schema"),
        BindingIndex::value(2),
    ) {
        SealOutcome::Sealed(kt) => kt,
        SealOutcome::DanglingRef(missing) => panic!("unexpected dangling ref `{missing}`"),
        SealOutcome::Rebind(e) => panic!("a parallel finalize must not Rebind: {e}"),
    };
    assert!(
        std::ptr::eq(sealed, again),
        "a parallel finalize returns the installed identity itself",
    );
    // A different statement declaring `Node` over different content is a redeclaration: the
    // filled member and the differing index route it to a fresh singleton, whose install collides.
    match finalize_nominal_member(
        scope,
        "Node",
        KKind::NewType,
        |_| SchemaSealResult::Ok(NominalSchema::NewType(Box::new(KType::Str))),
        BindingIndex::value(3),
    ) {
        SealOutcome::Rebind(e) => assert!(
            matches!(&e.kind, crate::machine::KErrorKind::Rebind { name } if name == "Node"),
            "expected Rebind naming Node, got {e}",
        ),
        SealOutcome::Sealed(kt) => panic!("expected Rebind on redeclaration, got Sealed({kt:?})"),
        SealOutcome::DanglingRef(missing) => panic!("unexpected dangling ref `{missing}`"),
    }
}

#[test]
fn constructor_apply_name_renders_surface_form() {
    use crate::machine::model::types::NominalSchema;
    let types = TypeRegistry::new();
    let set = RecursiveSet::singleton(
        "Wrap".into(),
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
    assert_eq!(app.name(&types), ":(Wrap {Type = Number})");
}
