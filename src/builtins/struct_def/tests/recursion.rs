//! Self-recursive and mutually-recursive struct elaboration.

use crate::builtins::test_support::{parse_one, run_one, run_root_silent};
use crate::machine::model::{KObject, KType};
use crate::machine::RuntimeArena;

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
