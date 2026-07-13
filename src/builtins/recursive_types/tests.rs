//! `RECURSIVE TYPES` block sealing: co-declared members seal into one shared
//! `RecursiveSet`, cross-references seal to `SetLocal` indices into that set, and the group
//! name binds the set handle. Exiting the block guarantees every forward reference resolved.

use crate::builtins::test_support::{parse_one, run, run_one_err, run_root_silent};
use crate::machine::core::run_root_storage;
use crate::machine::model::types::{NominalSchema, RecursiveSet};
use crate::machine::model::KType;
use crate::machine::{KErrorKind, Scope};

/// `(set, field-types)` of a sealed record-repr newtype member, read off its `SetRef` identity. Field
/// types carry raw `SetLocal` / `SetRef` leaves so assertions inspect the seal shape.
fn struct_set_and_fields<'a>(
    scope: &'a Scope<'a>,
    name: &str,
) -> (std::rc::Rc<RecursiveSet<'a>>, Vec<(String, KType<'a>)>) {
    match scope.resolve_type(name) {
        Some(KType::SetRef { set, index }) => {
            let member = set.member(*index);
            let borrow = member.schema();
            match borrow.as_ref() {
                Some(NominalSchema::NewType(repr)) => match repr.as_ref() {
                    KType::Record { fields: record, .. } => {
                        let fields = record.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                        (std::rc::Rc::clone(set), fields)
                    }
                    other => panic!("expected {name} to carry a record repr, got {other:?}"),
                },
                other => panic!("expected {name} to carry a NewType schema, got {other:?}"),
            }
        }
        other => panic!("expected {name} to be a SetRef identity in types, got {other:?}"),
    }
}

/// A mutually-recursive pair co-declared in a block seals into one shared set; each
/// cross-reference is a `SetLocal` into that set, and the members bind in the enclosing
/// scope.
#[test]
fn block_mutual_pair_seals_one_set_with_set_local_cross_refs() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "RECURSIVE TYPES Pair = (\n  NEWTYPE Aa = :{b :Bb}\n  NEWTYPE Bb = :{a :Aa}\n)",
    );
    let (a_set, a_fields) = struct_set_and_fields(scope, "Aa");
    let (b_set, b_fields) = struct_set_and_fields(scope, "Bb");
    assert!(
        std::rc::Rc::ptr_eq(&a_set, &b_set),
        "Aa and Bb share one RecursiveSet",
    );
    assert_eq!(a_set.len(), 2, "the block seals into a set of 2 members");
    let a_idx = a_set.index_of("Aa").unwrap();
    let b_idx = a_set.index_of("Bb").unwrap();
    assert_eq!(a_fields[0], ("b".to_string(), KType::SetLocal(b_idx)));
    assert_eq!(b_fields[0], ("a".to_string(), KType::SetLocal(a_idx)));
    assert!(scope.bindings().pending_types().is_empty());
}

/// The group name binds a `RecursiveGroup` handle over the members' shared set.
#[test]
fn block_group_name_binds_the_set_handle() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "RECURSIVE TYPES Pair = (\n  NEWTYPE Aa = :{b :Bb}\n  NEWTYPE Bb = :{a :Aa}\n)",
    );
    let (a_set, _) = struct_set_and_fields(scope, "Aa");
    match scope.resolve_type("Pair") {
        Some(KType::RecursiveGroup(set)) => assert!(
            std::rc::Rc::ptr_eq(set, &a_set),
            "Pair must handle the members' shared set",
        ),
        other => panic!("expected Pair to bind a RecursiveGroup handle, got {other:?}"),
    }
}

/// Three-way mutual recursion: one shared set of 3; each field is a `SetLocal` to the next.
#[test]
fn block_three_way_seals_one_set() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    run(
        scope,
        "RECURSIVE TYPES Trio = (\n  NEWTYPE Aa = :{b :Bb}\n  NEWTYPE Bb = :{c :Cc}\n  NEWTYPE Cc = :{a :Aa}\n)",
    );
    let (set, _) = struct_set_and_fields(scope, "Aa");
    assert_eq!(set.len(), 3);
    for (from, field, target) in [("Aa", "b", "Bb"), ("Bb", "c", "Cc"), ("Cc", "a", "Aa")] {
        let (from_set, fields) = struct_set_and_fields(scope, from);
        assert!(
            std::rc::Rc::ptr_eq(&from_set, &set),
            "{from} shares the set"
        );
        let target_idx = set.index_of(target).unwrap();
        assert_eq!(fields[0], (field.to_string(), KType::SetLocal(target_idx)));
    }
}

/// A non-declaration statement in the block body is a shape error, and nothing binds.
#[test]
fn block_body_rejects_non_declaration() {
    let region = run_root_storage();
    let scope = run_root_silent(&region);
    let err = run_one_err(
        scope,
        parse_one("RECURSIVE TYPES Grp = (\n  NEWTYPE Aa = :{x :Number}\n  LET y = 1\n)"),
    );
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
    let scope = run_root_silent(&region);
    run(scope, "RECURSIVE TYPES Grp = (NEWTYPE Aa = :{b :Nope})");
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
    let scope = run_root_silent(&region);
    let err = run_one_err(
        scope,
        parse_one(
            "RECURSIVE TYPES Grp = (\n  NEWTYPE Aa = :{x :Number}\n  NEWTYPE Aa = :{y :Number}\n)",
        ),
    );
    assert!(
        matches!(&err.kind, KErrorKind::ShapeError(m) if m.contains("duplicate member `Aa`")),
        "expected a duplicate-member shape error, got {err}",
    );
}
