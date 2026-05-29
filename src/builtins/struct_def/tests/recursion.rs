//! Self-recursive and mutually-recursive struct elaboration.

use crate::builtins::test_support::{parse_one, run_one, run_root_silent};
use crate::machine::model::{KObject, KType};
use crate::machine::RuntimeArena;

/// Self-recursive STRUCT: `STRUCT Tree = (children :(List Tree))` should elaborate
/// the field as `List(RecursiveRef("Tree"))` via the elaborator's binder-name
/// threading.
///
/// Disabled: a parameterized self-reference sub-Dispatches through the
/// standalone dispatcher, which has no SCC threading context — `Tree` reaches
/// the bare-Type-leaf fast lane and errors `UnboundName`. See
/// [roadmap/dispatch_fix/scc-aware-dispatcher-for-self-recursive-types.md](../../../../roadmap/dispatch_fix/scc-aware-dispatcher-for-self-recursive-types.md).
#[ignore = "blocked on SCC-aware dispatcher for self-recursive parameterized types"]
#[test]
fn recursive_struct_tree_elaborates_with_recursive_ref_on_field() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run_one(scope, parse_one("STRUCT Tree = (children :(List Tree))"));
    let data = scope.bindings().data();
    let (tree_obj, _) = *data.get("Tree").expect("Tree should be bound");
    match tree_obj {
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

/// Two unrelated STRUCTs in the same batch must not cross-pollinate
/// `RecursiveRef`: `Bb`'s `y :Aa` field must resolve to `UserType{Aa}`, not a
/// `RecursiveRef` — per-binder threaded-set seeding scopes the short-circuit to
/// the binder's own name.
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
    let b_fields = match data.get("Bb").map(|(o, _)| *o) {
        Some(KObject::StructType { fields, .. }) => fields.clone(),
        other => panic!("expected Bb to be a StructType, got {:?}", other.map(|o| o.ktype())),
    };
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
    let a_fields = match data.get("TreeA").map(|(o, _)| *o) {
        Some(KObject::StructType { fields, .. }) => fields.clone(),
        other => panic!("expected TreeA StructType, got {:?}", other.map(|o| o.ktype())),
    };
    let b_fields = match data.get("TreeB").map(|(o, _)| *o) {
        Some(KObject::StructType { fields, .. }) => fields.clone(),
        other => panic!("expected TreeB StructType, got {:?}", other.map(|o| o.ktype())),
    };
    // Cross-member references resolve to `UserType` (not `RecursiveRef`) because
    // SCC cycle-close pre-installs both identities before either finalizes.
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
        let fields = match data.get(from).map(|(o, _)| *o) {
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
