//! Per-declaration dispatch separation, wildcard slot admission, finalize idempotency.

use crate::builtins::test_support::{parse_one, run_one, run_root_silent};
use crate::machine::model::types::{NominalKind, NominalMember, ProjectedSchema, RecursiveSet};
use crate::machine::model::{KObject, KType};
use crate::machine::BindingIndex;
use crate::machine::RuntimeArena;

/// `finalize_struct` fills the member of a pre-installed `SetRef` (the seal pre-install),
/// and a second finalize against the now-filled member short-circuits. Pins the
/// seal-then-Combine-finish double-fire safety net under the type-only protocol.
#[test]
fn finalize_struct_idempotent_after_seal_pre_install() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    let scope_id = scope.id;
    // Mimic the seal pre-install: a `SetRef` to a pending (unfilled) member.
    let pre_set = std::rc::Rc::new(RecursiveSet::new(vec![NominalMember::pending(
        "Foo".into(),
        scope_id,
        NominalKind::Struct,
    )]));
    let pre_identity = KType::SetRef {
        set: std::rc::Rc::clone(&pre_set),
        index: 0,
    };
    scope.cycle_close_install_identity("Foo".into(), pre_identity, BindingIndex::nominal(0));
    // First finalize: fills the pre-installed member in place.
    let first = super::super::finalize_struct(
        scope,
        "Foo".into(),
        vec![("x".into(), KType::Number)],
        BindingIndex::nominal(0),
    );
    assert!(matches!(first, crate::machine::BodyResult::Value(_)));
    assert!(pre_set.member(0).is_filled());
    let stored = scope.resolve_type("Foo").expect("Foo identity in types");
    match stored {
        KType::SetRef { set, index } => match RecursiveSet::projected_schema(set, *index) {
            ProjectedSchema::Struct(fields) => {
                assert_eq!(fields.len(), 1);
                assert_eq!(fields.get("x"), Some(&KType::Number));
            }
            _ => panic!("expected a Struct schema"),
        },
        other => panic!("expected populated Struct SetRef, got {other:?}"),
    }
    // Second finalize observes the filled member and short-circuits without re-upsert.
    let second = super::super::finalize_struct(
        scope,
        "Foo".into(),
        vec![("x".into(), KType::Number)],
        BindingIndex::nominal(0),
    );
    match second {
        crate::machine::BodyResult::Value(KObject::KTypeValue(KType::SetRef {
            set,
            index,
            ..
        })) => {
            assert_eq!(set.member(*index).name, "Foo");
        }
        _ => panic!("expected short-circuit Value(KTypeValue(SetRef)) from finalize_struct"),
    }
    assert!(
        scope.bindings().data().get("Foo").is_none(),
        "type-only finalize must not write a value-side carrier",
    );
}

/// Two STRUCTs declared in the same scope share `scope_id` on their members but seal into
/// distinct sets — set-pointer identity separates per-declaration identity within a scope.
#[test]
fn struct_pair_same_scope_distinct_names_share_scope_id() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run_one(scope, parse_one("STRUCT Foo = (x :Number)"));
    run_one(scope, parse_one("STRUCT Bar = (x :Number)"));
    let foo_id = match scope.resolve_type("Foo") {
        Some(KType::SetRef { set, index }) => {
            assert_eq!(set.member(*index).name, "Foo");
            set.member(*index).scope_id
        }
        other => panic!("expected SetRef Foo identity, got {other:?}"),
    };
    let bar_id = match scope.resolve_type("Bar") {
        Some(KType::SetRef { set, index }) => {
            assert_eq!(set.member(*index).name, "Bar");
            set.member(*index).scope_id
        }
        other => panic!("expected SetRef Bar identity, got {other:?}"),
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
    let foo_result = run_one(scope, parse_one("PICK (Foo {a = 1})"));
    match foo_result {
        KObject::KString(s) => assert_eq!(s, "foo"),
        other => panic!("expected \"foo\", got {:?}", other.ktype()),
    }
    let bar_result = run_one(scope, parse_one("PICK (Bar {a = 1})"));
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
    let foo_result = run_one(scope, parse_one("PICK (Foo {a = 1})"));
    let bar_result = run_one(scope, parse_one("PICK (Bar {a = 1})"));
    match (foo_result, bar_result) {
        (KObject::KString(a), KObject::KString(b)) => {
            assert_eq!(a, "any");
            assert_eq!(b, "any");
        }
        _ => panic!("expected both PICK calls to return \"any\""),
    }
}
