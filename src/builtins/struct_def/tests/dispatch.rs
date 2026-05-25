//! Per-declaration dispatch separation, wildcard slot admission, finalize idempotency.

use crate::builtins::test_support::{parse_one, run_one, run_root_silent};
use crate::machine::model::{KObject, KType};
use crate::machine::RuntimeArena;

/// `finalize_struct` is idempotent when both `bindings.types[name]` and
/// `bindings.data[name]` are already populated. Pins the defensive guard at the
/// top of `finalize_struct` against a future refactor that might silently
/// regress the cycle-close-then-Combine-finish double-fire safety net.
#[test]
fn finalize_struct_is_idempotent_when_both_maps_populated() {
    use crate::machine::model::types::UserTypeKind;
    use std::rc::Rc;
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    // Pre-seed both maps to mimic the cycle-close-then-finalize state.
    let scope_id = scope.id;
    let pre_carrier: &KObject<'_> = arena.alloc(KObject::StructType {
        name: "Foo".into(),
        scope_id,
        fields: Rc::new(vec![("x".into(), KType::Number)]),
    });
    let pre_identity = KType::UserType {
        kind: UserTypeKind::Struct,
        scope_id,
        name: "Foo".into(),
    };
    scope.register_nominal("Foo".into(), pre_identity, pre_carrier).unwrap();
    // Call finalize_struct directly — it must short-circuit to the existing carrier.
    let outcome = super::super::finalize_struct(
        scope,
        "Foo".into(),
        vec![("x".into(), KType::Number)],
    );
    match outcome {
        crate::machine::BodyResult::Value(obj) => {
            assert!(std::ptr::eq(obj, pre_carrier),
                "finalize_struct must return the pre-installed carrier pointer");
        }
        _ => panic!("expected Value variant from finalize_struct"),
    }
}

/// Stage 3.0c identity-field invariant: two STRUCTs declared in the same scope
/// share `scope_id` (they're both bound on the same parent scope) but carry
/// distinct `name`s. This is the per-declaration identity the 3.1 `ktype()` flip
/// reads — `Foo` and `Bar` lower to distinct `KType::UserType { name: .., scope_id: .. }`
/// even though they sit in the same scope, because `name` separates them.
#[test]
fn struct_pair_same_scope_distinct_names_share_scope_id() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run_one(scope, parse_one("STRUCT Foo = (x :Number)"));
    run_one(scope, parse_one("STRUCT Bar = (x :Number)"));
    let data = scope.bindings().data();
    let foo_id = match data.get("Foo") {
        Some(KObject::StructType { scope_id, name, .. }) => {
            assert_eq!(name, "Foo");
            *scope_id
        }
        other => panic!("expected StructType Foo, got {:?}", other.map(|o| o.ktype())),
    };
    let bar_id = match data.get("Bar") {
        Some(KObject::StructType { scope_id, name, .. }) => {
            assert_eq!(name, "Bar");
            *scope_id
        }
        other => panic!("expected StructType Bar, got {:?}", other.map(|o| o.ktype())),
    };
    assert_eq!(foo_id, bar_id, "same-scope STRUCTs must share scope_id");
}

/// Stage 3.1 impact: two STRUCTs declared at the same scope with identical field shapes
/// have distinct per-declaration identity. Two `FN (PICK x: Foo)` and
/// `FN (PICK x: Bar)` overloads coexist (pre-3.1 collapsed to `DuplicateOverload`
/// because both slot types lowered to `KType::Struct`), and dispatching on a
/// `Foo`-typed value selects the `Foo` body — same for `Bar`.
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

/// Wildcard slot `Struct` admits any struct carrier regardless of declaring schema —
/// the `AnyUserType { kind: Struct }` arm. Both `Foo` and `Bar` values lower to
/// distinct `UserType`s, but both refine `AnyUserType { kind: Struct }`.
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

/// STRUCT finalize dual-writes the identity into `bindings.types` AND the carrier
/// into `bindings.data` via `register_nominal`. Both maps must hold matching entries
/// for the same name after declaration.
#[test]
fn struct_dual_writes_to_types_and_data() {
    use crate::machine::model::types::UserTypeKind;
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run_one(scope, parse_one("STRUCT Point = (x :Number, y :Number)"));
    let types = scope.bindings().types();
    let kt = types.get("Point").expect("Point should be in bindings.types");
    assert!(matches!(
        **kt,
        KType::UserType { kind: UserTypeKind::Struct, ref name, .. } if name == "Point"
    ));
    drop(types);
    let data = scope.bindings().data();
    let obj = data.get("Point").expect("Point should be in bindings.data");
    assert!(matches!(obj, KObject::StructType { name, .. } if name == "Point"));
}
