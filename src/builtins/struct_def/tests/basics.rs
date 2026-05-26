//! pre_run extraction, type registration, field ordering, schema-rejection errors.

use crate::builtins::test_support::{parse_one, run_one, run_one_err, run_root_silent};
use crate::machine::model::{KObject, KType};
use crate::machine::{KErrorKind, RuntimeArena};

/// Smoke test for STRUCT's pre_run extractor: structural extraction of the `Type(_)`
/// token at `parts[1]`.
#[test]
fn pre_run_extracts_struct_name() {
    let expr = parse_one("STRUCT Point = (x :Number, y :Number)");
    let name = super::super::pre_run(&expr);
    assert_eq!(name.as_deref(), Some("Point"));
}

#[test]
fn struct_named_registers_type_in_scope() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    let result = run_one(
        scope,
        parse_one("STRUCT Point = (x :Number, y :Number)"),
    );
    match result {
        KObject::StructType { name, fields, .. } => {
            assert_eq!(name, "Point");
            assert_eq!(fields.len(), 2);
            assert_eq!(fields[0], ("x".to_string(), KType::Number));
            assert_eq!(fields[1], ("y".to_string(), KType::Number));
        }
        other => panic!("expected StructType, got {:?}", other.ktype()),
    }
    let data = scope.bindings().data();
    let (entry, _) = data.get("Point").expect("Point should be bound in scope");
    assert!(matches!(entry, KObject::StructType { .. }));
}

#[test]
fn struct_returns_type_value() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    let result = run_one(scope, parse_one("STRUCT Point = (x :Number, y :Number)"));
    assert_eq!(result.ktype(), KType::Type);
}

#[test]
fn struct_preserves_field_order() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run_one(scope, parse_one("STRUCT Backwards = (b :Number, a :Number)"));
    let data = scope.bindings().data();
    let (entry, _) = *data.get("Backwards").unwrap();
    match entry {
        KObject::StructType { fields, .. } => {
            assert_eq!(fields[0].0, "b", "first field should be `b` (declaration order)");
            assert_eq!(fields[1].0, "a");
        }
        _ => panic!("expected StructType"),
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

/// RAII pending-types lifecycle: a body-Err arm (here: unknown type name in
/// the schema, which routes through `FieldListOutcome::Err`) must leave
/// `bindings.pending_types` empty. With the guard the cleanup is
/// unconditional; this test pins the property against a regression that
/// shadows / forgets the guard on the early-return path.
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
    // Under the Design-B sigil regime, typed fields parse as `[Identifier, Type]`
    // PAIRS. An odd number of parts (a name without its type slot) is rejected by
    // the pair-list walker.
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    let err = run_one_err(scope, parse_one("STRUCT Pair = (x :Number y)"));
    assert!(
        matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("pair") || msg.contains("multiple of 2")),
        "expected ShapeError on odd part count, got {err}",
    );
}
