use crate::builtins::test_support::{parse_one, run_one, run_one_err, run_root_silent};
use crate::machine::model::{KObject, KType};
use crate::machine::{KErrorKind, RuntimeArena};

/// Smoke test for STRUCT's pre_run extractor: structural extraction of the `Type(_)`
/// token at `parts[1]`.
#[test]
fn pre_run_extracts_struct_name() {
    let expr = parse_one("STRUCT Point = (x :Number, y :Number)");
    let name = super::pre_run(&expr);
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
    let entry = data.get("Point").expect("Point should be bound in scope");
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
    match data.get("Backwards").unwrap() {
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

/// Phase 3 — self-recursive STRUCT: `STRUCT Tree = (children: List<Tree>)` elaborates
/// with the field type carrying `KType::RecursiveRef("Tree")` inside `KType::List(...)`.
/// The elaborator's threaded set seeded with the binder's own name short-circuits the
/// self-reference to `RecursiveRef` rather than parking on the binder's placeholder.
#[test]
fn recursive_struct_tree_elaborates_with_recursive_ref_on_field() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run_one(scope, parse_one("STRUCT Tree = (children :(List Tree))"));
    let data = scope.bindings().data();
    match data.get("Tree").expect("Tree should be bound") {
        KObject::StructType { name, fields, .. } => {
            assert_eq!(name, "Tree");
            assert_eq!(fields.len(), 1);
            assert_eq!(fields[0].0, "children");
            assert_eq!(
                fields[0].1,
                KType::List(Box::new(KType::RecursiveRef("Tree".into()))),
            );
        }
        other => panic!("expected StructType, got {:?}", other.ktype()),
    }
}

/// Mutually recursive STRUCTs. `STRUCT TreeA = (b: TreeB)` and
/// `STRUCT TreeB = (a: TreeA)` submitted in the same batch must both finalize.
/// Stage 3.2 SCC pre-registration installs each binder's identity into
/// `bindings.types` synchronously at cycle-close, so cross-member references
/// resolve to `KType::UserType` directly — no `RecursiveRef` wrap inside SCC
/// members.
/// Sanity check that two unrelated STRUCTs in the same batch don't
/// spuriously cross-pollinate `RecursiveRef`. `STRUCT A = (x: Number)`,
/// `STRUCT B = (y: A)` — B's field references A, which is non-recursive; B's schema    /// must record the resolved `KType` for `y` (post-3.1: `KType::UserType { kind:
/// Struct, .. }` from Aa's identity), never a `RecursiveRef`. Per-binder
/// threaded-set seeding handles this — only the binder's own name is in its
/// threaded set.
#[test]
fn mutual_non_recursive_pair_does_not_wrap_either() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    use crate::machine::execute::Scheduler;
    use crate::parse::parse;
    let mut sched = Scheduler::new();
    for e in parse("STRUCT Aa = (x :Number)\nSTRUCT Bb = (y :Aa)").unwrap() {
        sched.add_dispatch(e, scope);
    }
    sched.execute().unwrap();
    let data = scope.bindings().data();
    let b_fields = match data.get("Bb") {
        Some(KObject::StructType { fields, .. }) => fields.clone(),
        other => panic!("expected Bb to be a StructType, got {:?}", other.map(|o| o.ktype())),
    };
    // `y`'s recorded KType is whatever the elaborator pulls out of `Aa`'s binding —
    // post-3.1 `KType::UserType { kind: Struct, name: "Aa", .. }` from the dual-
    // write — not `RecursiveRef`.
    assert_eq!(b_fields[0].0, "y");
    assert!(
        !matches!(b_fields[0].1, KType::RecursiveRef(_)),
        "Bb's `y` field must not be wrapped in RecursiveRef, got {:?}",
        b_fields[0].1,
    );
}

#[test]
fn mutually_recursive_struct_pair() {
    use crate::machine::model::types::UserTypeKind;
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    use crate::machine::execute::Scheduler;
    use crate::parse::parse;
    let mut sched = Scheduler::new();
    for e in parse("STRUCT TreeA = (b :TreeB)\nSTRUCT TreeB = (a :TreeA)").unwrap() {
        sched.add_dispatch(e, scope);
    }
    sched.execute().unwrap();
    let data = scope.bindings().data();
    let a_fields = match data.get("TreeA") {
        Some(KObject::StructType { fields, .. }) => fields.clone(),
        other => panic!("expected TreeA StructType, got {:?}", other.map(|o| o.ktype())),
    };
    let b_fields = match data.get("TreeB") {
        Some(KObject::StructType { fields, .. }) => fields.clone(),
        other => panic!("expected TreeB StructType, got {:?}", other.map(|o| o.ktype())),
    };
    // Each member's field references the OTHER as `UserType` (not `RecursiveRef`):
    // SCC cycle-close pre-installed both identities so cross-member resolution
    // returns the named identity directly.
    assert_eq!(a_fields[0].0, "b");
    assert!(
        matches!(&a_fields[0].1, KType::UserType { kind: UserTypeKind::Struct, name, .. } if name == "TreeB"),
        "TreeA.b expected UserType{{TreeB}}, got {:?}",
        a_fields[0].1,
    );
    assert_eq!(b_fields[0].0, "a");
    assert!(
        matches!(&b_fields[0].1, KType::UserType { kind: UserTypeKind::Struct, name, .. } if name == "TreeA"),
        "TreeB.a expected UserType{{TreeA}}, got {:?}",
        b_fields[0].1,
    );
    // Pending-types entries are drained after cycle-close + each member's finalize.
    drop(data);
    assert!(scope.bindings().pending_types().is_empty());
}

/// Three-way mutual recursion: A → B → C → A. Exercises SCC DFS past depth 2.
#[test]
fn three_way_mutual_recursion_struct_chain() {
    use crate::machine::model::types::UserTypeKind;
    use crate::machine::execute::Scheduler;
    use crate::parse::parse;
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    let mut sched = Scheduler::new();
    for e in parse("STRUCT Aaa = (b :Bbb)\nSTRUCT Bbb = (c :Ccc)\nSTRUCT Ccc = (a :Aaa)").unwrap() {
        sched.add_dispatch(e, scope);
    }
    sched.execute().unwrap();
    let data = scope.bindings().data();
    for (from, expected_field, expected_target) in
        [("Aaa", "b", "Bbb"), ("Bbb", "c", "Ccc"), ("Ccc", "a", "Aaa")]
    {
        let fields = match data.get(from) {
            Some(KObject::StructType { fields, .. }) => fields.clone(),
            other => panic!("expected {from} StructType, got {:?}", other.map(|o| o.ktype())),
        };
        assert_eq!(fields[0].0, expected_field);
        assert!(
            matches!(&fields[0].1, KType::UserType { kind: UserTypeKind::Struct, name, .. } if name == expected_target),
            "{from}.{expected_field} expected UserType{{{expected_target}}}, got {:?}",
            fields[0].1,
        );
    }
}

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
    let pre_carrier: &KObject<'_> = arena.alloc_object(KObject::StructType {
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
    let outcome = super::finalize_struct(
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
