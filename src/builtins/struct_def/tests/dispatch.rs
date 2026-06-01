//! Per-declaration dispatch separation, wildcard slot admission, finalize idempotency.

use crate::builtins::test_support::{parse_one, run_one, run_root_silent};
use crate::machine::model::types::UserTypeKind;
use crate::machine::model::{KObject, KType};
use crate::machine::BindingIndex;
use crate::machine::RuntimeArena;

/// `finalize_struct` is idempotent against a cycle-close payload-empty pre-install:
/// finalize upserts the schema-bearing identity over it, and a second finalize against
/// the now-populated identity short-circuits. Pins the cycle-close-then-Combine-finish
/// double-fire safety net under the type-only (no value-side carrier) protocol.
#[test]
fn finalize_struct_idempotent_after_cycle_close_pre_install() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    let scope_id = scope.id;
    // Mimic cycle-close: install the payload-empty identity into `types` only.
    let pre_identity = KType::UserType {
        kind: UserTypeKind::struct_sentinel(),
        scope_id,
        name: "Foo".into(),
    };
    scope.cycle_close_install_identity("Foo".into(), pre_identity, BindingIndex::nominal(0));
    // First finalize: upserts the schema-bearing identity, replacing the empty payload.
    let first = super::super::finalize_struct(
        scope,
        "Foo".into(),
        vec![("x".into(), KType::Number)],
        BindingIndex::nominal(0),
    );
    assert!(matches!(first, crate::machine::BodyResult::Value(_)));
    let stored = scope.resolve_type("Foo").expect("Foo identity in types");
    match stored {
        KType::UserType {
            kind: UserTypeKind::Struct { fields },
            ..
        } => {
            assert_eq!(fields.len(), 1);
            assert_eq!(fields.get("x"), Some(&KType::Number));
        }
        other => panic!("expected populated Struct identity, got {other:?}"),
    }
    // Second finalize observes the populated payload and short-circuits without re-upsert.
    let second = super::super::finalize_struct(
        scope,
        "Foo".into(),
        vec![("x".into(), KType::Number)],
        BindingIndex::nominal(0),
    );
    match second {
        crate::machine::BodyResult::Value(KObject::KTypeValue(KType::UserType {
            name, ..
        })) => {
            assert_eq!(name, "Foo");
        }
        _ => panic!("expected short-circuit Value(KTypeValue(UserType)) from finalize_struct"),
    }
    assert!(
        scope.bindings().data().get("Foo").is_none(),
        "type-only finalize must not write a value-side carrier",
    );
}

/// Two STRUCTs declared in the same scope share `scope_id` but carry distinct
/// `name`s — `name` separates per-declaration identity within a scope.
#[test]
fn struct_pair_same_scope_distinct_names_share_scope_id() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run_one(scope, parse_one("STRUCT Foo = (x :Number)"));
    run_one(scope, parse_one("STRUCT Bar = (x :Number)"));
    let foo_id = match scope.resolve_type("Foo") {
        Some(KType::UserType { scope_id, name, .. }) => {
            assert_eq!(name, "Foo");
            *scope_id
        }
        other => panic!("expected UserType Foo identity, got {other:?}"),
    };
    let bar_id = match scope.resolve_type("Bar") {
        Some(KType::UserType { scope_id, name, .. }) => {
            assert_eq!(name, "Bar");
            *scope_id
        }
        other => panic!("expected UserType Bar identity, got {other:?}"),
    };
    assert_eq!(foo_id, bar_id, "same-scope STRUCTs must share scope_id");
}

/// Two STRUCTs with identical field shapes have distinct per-declaration
/// identity: `FN (PICK x: Foo)` and `FN (PICK x: Bar)` coexist, and dispatching
/// on a `Foo`-typed value selects the `Foo` body.
#[test]
fn per_declaration_dispatch_separates_overloads() {
    use crate::builtins::test_support::run;
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "STRUCT Foo = (a :Number)\n\
         STRUCT Bar = (a :Number)\n\
         FN (PICK x :Foo) -> Str = (\"foo\")\n\
         FN (PICK x :Bar) -> Str = (\"bar\")",
    );
    let foo_result = run_one(scope, parse_one("PICK (Foo (a = 1))"));
    match foo_result {
        KObject::KString(s) => assert_eq!(s, "foo"),
        other => panic!("expected \"foo\", got {:?}", other.ktype()),
    }
    let bar_result = run_one(scope, parse_one("PICK (Bar (a = 1))"));
    match bar_result {
        KObject::KString(s) => assert_eq!(s, "bar"),
        other => panic!("expected \"bar\", got {:?}", other.ktype()),
    }
}

/// Wildcard slot `Struct` admits any struct carrier: both `Foo` and `Bar`
/// values lower to distinct `UserType`s but both refine `AnyUserType { kind: Struct }`.
#[test]
fn wildcard_struct_slot_admits_any_struct_carrier() {
    use crate::builtins::test_support::run;
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run(
        scope,
        "STRUCT Foo = (a :Number)\n\
         STRUCT Bar = (a :Number)\n\
         FN (PICK x :Struct) -> Str = (\"any\")",
    );
    let foo_result = run_one(scope, parse_one("PICK (Foo (a = 1))"));
    let bar_result = run_one(scope, parse_one("PICK (Bar (a = 1))"));
    match (foo_result, bar_result) {
        (KObject::KString(a), KObject::KString(b)) => {
            assert_eq!(a, "any");
            assert_eq!(b, "any");
        }
        _ => panic!("expected both PICK calls to return \"any\""),
    }
}
