//! `RECURSIVE TYPES` block sealing: co-declared members that reference each other seal into one
//! strongly-connected component, each cross-reference sealing to the referent's absolute member
//! handle, and the group name binding a `Group` handle over the declared members. Exiting the
//! block guarantees every forward reference resolved.

use crate::builtins::test_support::{parse_one, TestRun};
use crate::machine::model::KType;
use crate::machine::model::{NodeSchema, TypeDigest, TypeNode, TypeRegistry};
use crate::machine::run_root_storage;
use crate::machine::{KErrorKind, Scope};

/// `(scc-digest, scc-size, field-types)` of a sealed record-repr newtype member, read off its
/// `SetMember` identity. The member's SCC digest and component size witness which members seal
/// together; the field types carry the absolute member handles the sealed schema references, so
/// assertions inspect the seal shape.
fn member_scc_and_fields(
    scope: &Scope<'_>,
    types: &TypeRegistry,
    name: &str,
) -> (TypeDigest, usize, Vec<(String, KType)>) {
    let handle = scope
        .resolve_type(name)
        .unwrap_or_else(|| panic!("expected {name} to be a type in scope"));
    match types.node(handle) {
        TypeNode::SetMember {
            scc_digest,
            scc_size,
            schema,
            ..
        } => match schema {
            NodeSchema::NewType(repr) => match types.node(repr) {
                TypeNode::Record { fields } => {
                    let fields = fields.iter().map(|(k, v)| (k.clone(), *v)).collect();
                    (scc_digest, scc_size, fields)
                }
                _ => panic!("expected {name} to carry a record repr, got {repr:?}"),
            },
            _ => panic!("expected {name} to carry a NewType schema for {handle:?}"),
        },
        _ => panic!("expected {name} to be a SetMember identity in types, got {handle:?}"),
    }
}

/// A mutually-recursive pair co-declared in a block seals into one shared component of two; each
/// cross-reference seals to the referent member's absolute handle, and the members bind in the
/// enclosing scope.
#[test]
fn block_mutual_pair_seals_one_component_with_member_handle_cross_refs() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    test_run.run("RECURSIVE TYPES Pair = (\n  NEWTYPE Aa = :{b :Bb}\n  NEWTYPE Bb = :{a :Aa}\n)");
    let types = test_run.types();
    let (a_scc, a_size, a_fields) = member_scc_and_fields(scope, types, "Aa");
    let (b_scc, b_size, b_fields) = member_scc_and_fields(scope, types, "Bb");
    assert_eq!(a_scc, b_scc, "Aa and Bb seal into one component");
    assert_eq!(a_size, 2, "the block seals into a component of 2 members");
    assert_eq!(b_size, 2);
    let aa_handle = scope.resolve_type("Aa").unwrap();
    let bb_handle = scope.resolve_type("Bb").unwrap();
    assert_eq!(a_fields[0], ("b".to_string(), bb_handle));
    assert_eq!(b_fields[0], ("a".to_string(), aa_handle));
    assert!(scope.bindings().pending_types().is_empty());
}

/// The group name binds a `Group` handle over the block's declared members.
#[test]
fn block_group_name_binds_the_group_handle() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    test_run.run("RECURSIVE TYPES Pair = (\n  NEWTYPE Aa = :{b :Bb}\n  NEWTYPE Bb = :{a :Aa}\n)");
    let types = test_run.types();
    let aa = scope.resolve_type("Aa").unwrap();
    let bb = scope.resolve_type("Bb").unwrap();
    match scope.resolve_type("Pair") {
        Some(handle) => match types.node(handle) {
            TypeNode::Group { members } => assert_eq!(
                members,
                vec![aa, bb],
                "Pair's Group spans its declared members in order",
            ),
            _ => panic!("expected Pair to bind a Group handle, got {handle:?}"),
        },
        None => panic!("expected Pair to bind a Group handle"),
    }
}

/// Three-way mutual recursion: one shared component of 3; each field references the next member's
/// absolute handle.
#[test]
fn block_three_way_seals_one_component() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    test_run.run(
        "RECURSIVE TYPES Trio = (\n  NEWTYPE Aa = :{b :Bb}\n  NEWTYPE Bb = :{c :Cc}\n  NEWTYPE Cc = :{a :Aa}\n)",
    );
    let types = test_run.types();
    let (scc, size, _) = member_scc_and_fields(scope, types, "Aa");
    assert_eq!(size, 3);
    for (from, field, target) in [("Aa", "b", "Bb"), ("Bb", "c", "Cc"), ("Cc", "a", "Aa")] {
        let (from_scc, _, fields) = member_scc_and_fields(scope, types, from);
        assert_eq!(from_scc, scc, "{from} shares the component");
        let target_handle = scope.resolve_type(target).unwrap();
        assert_eq!(fields[0], (field.to_string(), target_handle));
    }
}

/// A non-declaration statement in the block body is a shape error, and nothing binds.
#[test]
fn block_body_rejects_non_declaration() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    let err = test_run.run_one_err(parse_one(
        "RECURSIVE TYPES Grp = (\n  NEWTYPE Aa = :{x :Number}\n  LET y = 1\n)",
    ));
    assert!(
        matches!(&err.kind, KErrorKind::ShapeError(m) if m.contains("UNION / NEWTYPE")),
        "expected a member-kind shape error, got {err}",
    );
    assert!(scope.resolve_type("Grp").is_none(), "Grp must not bind");
    assert!(
        scope.resolve_type("Aa").is_none(),
        "no member binds on a malformed block",
    );
}

/// A member referencing a name outside the group fails to seal, and neither the member nor
/// the group handle binds (the block guarantees resolution at its boundary).
#[test]
fn block_member_referencing_non_member_does_not_bind() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let scope = test_run.scope;
    test_run.run("RECURSIVE TYPES Grp = (NEWTYPE Aa = :{b :Nope})");
    assert!(
        scope.resolve_type("Aa").is_none(),
        "Aa must not bind when its schema references an unresolved name",
    );
    assert!(
        scope.resolve_type("Grp").is_none(),
        "the group handle must not bind"
    );
}

/// A duplicate member name in the body is a shape error.
#[test]
fn block_rejects_duplicate_member() {
    let region = run_root_storage();
    let mut test_run = TestRun::silent(&region);
    let err = test_run.run_one_err(parse_one(
        "RECURSIVE TYPES Grp = (\n  NEWTYPE Aa = :{x :Number}\n  NEWTYPE Aa = :{y :Number}\n)",
    ));
    assert!(
        matches!(&err.kind, KErrorKind::ShapeError(m) if m.contains("duplicate member `Aa`")),
        "expected a duplicate-member shape error, got {err}",
    );
}
