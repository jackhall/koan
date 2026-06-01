//! binder_name extraction, type registration, field ordering, schema-rejection errors.

use crate::builtins::test_support::{parse_one, run_one, run_one_err, run_root_silent};
use crate::machine::model::types::UserTypeKind;
use crate::machine::model::{KObject, KType};
use crate::machine::{KErrorKind, RuntimeArena};

#[test]
fn binder_name_extracts_struct_name() {
    let expr = parse_one("STRUCT Point = (x :Number, y :Number)");
    let name = super::super::binder_name(&expr);
    assert_eq!(name.as_deref(), Some("Point"));
}

#[test]
fn struct_named_registers_type_in_scope() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    // STRUCT is type-only now: the declaration yields a `KTypeValue(UserType)` whose
    // `Struct { fields }` payload carries the schema, and registers it into `types`.
    let result = run_one(scope, parse_one("STRUCT Point = (x :Number, y :Number)"));
    match result {
        KObject::KTypeValue(KType::UserType {
            kind: UserTypeKind::Struct { fields },
            name,
            ..
        }) => {
            assert_eq!(name, "Point");
            assert_eq!(fields.len(), 2);
            assert_eq!(
                fields.keys().map(String::as_str).collect::<Vec<_>>(),
                ["x", "y"]
            );
            assert_eq!(fields.get("x"), Some(&KType::Number));
            assert_eq!(fields.get("y"), Some(&KType::Number));
        }
        other => panic!(
            "expected KTypeValue(UserType Struct), got {:?}",
            other.ktype()
        ),
    }
    let kt = scope
        .resolve_type("Point")
        .expect("Point should be in types");
    assert!(matches!(
        kt,
        KType::UserType {
            kind: UserTypeKind::Struct { .. },
            ..
        }
    ));
    assert!(
        scope.bindings().data().get("Point").is_none(),
        "STRUCT must not write a value-side carrier into data",
    );
}

#[test]
fn struct_returns_type_value() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    let result = run_one(scope, parse_one("STRUCT Point = (x :Number, y :Number)"));
    // The declaration result is a first-class type value; its `ktype()` reports the
    // `TypeExprRef` dispatch-position marker (like every other `KTypeValue` carrier).
    assert_eq!(result.ktype(), KType::TypeExprRef);
}

#[test]
fn struct_preserves_field_order() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run_one(
        scope,
        parse_one("STRUCT Backwards = (b :Number, a :Number)"),
    );
    let kt = scope.resolve_type("Backwards").expect("Backwards in types");
    match kt {
        KType::UserType {
            kind: UserTypeKind::Struct { fields },
            ..
        } => {
            let names: Vec<&str> = fields.keys().map(String::as_str).collect();
            assert_eq!(
                names[0], "b",
                "first field should be `b` (declaration order)"
            );
            assert_eq!(names[1], "a");
        }
        _ => panic!("expected UserType Struct identity for Backwards"),
    }
}

#[test]
fn struct_rejects_unknown_type_name() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    let err = run_one_err(scope, parse_one("STRUCT Bad = (a :Bogus)"));
    assert!(
        matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("Bogus")),
        "expected ShapeError mentioning Bogus, got {err}",
    );
}

/// A body-Err arm must leave `bindings.pending_types` empty — pins against a
/// regression that shadows or forgets the RAII guard on the early-return path.
#[test]
fn struct_err_arm_drops_pending_types_entry() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    let _ = run_one_err(scope, parse_one("STRUCT Bad = (a :Bogus)"));
    assert!(
        scope.bindings().pending_types().is_empty(),
        "pending_types must be empty after a STRUCT body Err arm",
    );
}

#[test]
fn struct_rejects_empty_schema() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    let err = run_one_err(scope, parse_one("STRUCT Empty = ()"));
    assert!(
        matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("at least one field")),
        "expected ShapeError on empty schema, got {err}",
    );
}

#[test]
fn struct_rejects_duplicate_field() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    let err = run_one_err(scope, parse_one("STRUCT Pair = (x :Number, x :Str)"));
    assert!(
        matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("duplicate") && msg.contains("`x`")),
        "expected ShapeError on duplicate field, got {err}",
    );
}

#[test]
fn struct_rejects_odd_part_count() {
    // Typed fields parse as `[Identifier, Type]` pairs; a name without its type
    // slot is rejected by the pair-list walker.
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    let err = run_one_err(scope, parse_one("STRUCT Pair = (x :Number y)"));
    assert!(
        matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("pair") || msg.contains("multiple of 2")),
        "expected ShapeError on odd part count, got {err}",
    );
}
