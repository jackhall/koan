//! Self-recursive and mutually-recursive struct elaboration, against the `RecursiveSet`
//! sealing model: every nominal type is a `SetRef` into some set; intra-group references
//! are `SetLocal` indices into the *same* set, cross-group references are `SetRef`s.

use crate::builtins::test_support::{parse_one, run_one, run_root_silent};
use crate::machine::model::types::{NominalSchema, RecursiveSet};
use crate::machine::model::KType;
use crate::machine::{RuntimeArena, Scope};

/// The `(set, field-types)` of a STRUCT, read off its sealed member. `field-types` carry
/// raw `SetLocal` / `SetRef` leaves (un-projected) so assertions can inspect the seal shape.
fn struct_set_and_fields<'a>(
    scope: &'a Scope<'a>,
    name: &str,
) -> (std::rc::Rc<RecursiveSet<'a>>, Vec<(String, KType<'a>)>) {
    match scope.resolve_type(name) {
        Some(KType::SetRef { set, index }) => {
            let member = set.member(*index);
            let borrow = member.schema();
            match borrow.as_ref() {
                Some(NominalSchema::Struct(record)) => {
                    let fields = record.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                    (std::rc::Rc::clone(set), fields)
                }
                other => panic!("expected {name} to carry a Struct schema, got {other:?}"),
            }
        }
        other => panic!("expected {name} to be a SetRef identity in types, got {other:?}"),
    }
}

/// Self-recursive STRUCT, keyworded sigil: `STRUCT Tree = (children :(LIST OF Tree))`.
/// `Tree` seals into a singleton set; its `children` field references the set's own member,
/// so the sealed field is `List(SetLocal(0))` — the self-edge.
#[test]
fn recursive_struct_tree_keyworded_list_of_seals_to_set_local() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run_one(scope, parse_one("STRUCT Tree = (children :(LIST OF Tree))"));
    let (set, fields) = struct_set_and_fields(scope, "Tree");
    assert_eq!(
        set.len(),
        1,
        "a self-recursive type seals into a singleton set"
    );
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].0, "children");
    assert_eq!(
        fields[0].1,
        KType::List(Box::new(KType::SetLocal(0))),
        "children must seal its self-reference to SetLocal(0)",
    );
}

/// Two unrelated STRUCTs in the same batch must not cross-pollinate: `Bb`'s `y :Aa` field
/// resolves to a `SetRef` into `Aa`'s *own* set (a cross-group reference), NOT a self-edge
/// `SetLocal` and NOT a member of `Bb`'s set.
#[test]
fn mutual_non_recursive_pair_does_not_self_ref() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    use crate::machine::execute::Scheduler;
    use crate::parse::parse;
    let mut sched = Scheduler::new();
    for e in parse("STRUCT Aa = (x :Number)\nSTRUCT Bb = (y :Aa)").unwrap() {
        sched.add_dispatch(e, scope);
    }
    sched.execute().unwrap();
    let (aa_set, _) = struct_set_and_fields(scope, "Aa");
    let (bb_set, b_fields) = struct_set_and_fields(scope, "Bb");
    assert_eq!(b_fields[0].0, "y");
    match &b_fields[0].1 {
        KType::SetRef { set, index } => {
            assert!(
                std::rc::Rc::ptr_eq(set, &aa_set),
                "Bb.y must reference Aa's own set",
            );
            assert!(
                !std::rc::Rc::ptr_eq(set, &bb_set),
                "Bb.y must NOT be a member of Bb's set",
            );
            assert_eq!(set.member(*index).name, "Aa");
        }
        other => panic!("Bb.y expected a SetRef into Aa's set, got {other:?}"),
    }
    assert!(matches!(b_fields[0].1, KType::SetRef { .. }));
}

/// Mutually recursive `TreeA` ↔ `TreeB`: one shared set of 2 members. Each cross-reference
/// (`TreeA.b`, `TreeB.a`) is a `SetLocal` index into the *same* set pointing at the sibling.
#[test]
fn mutually_recursive_struct_pair() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    use crate::machine::execute::Scheduler;
    use crate::parse::parse;
    let mut sched = Scheduler::new();
    for e in parse("STRUCT TreeA = (b :TreeB)\nSTRUCT TreeB = (a :TreeA)").unwrap() {
        sched.add_dispatch(e, scope);
    }
    sched.execute().unwrap();
    let (a_set, a_fields) = struct_set_and_fields(scope, "TreeA");
    let (b_set, b_fields) = struct_set_and_fields(scope, "TreeB");
    // Both members live in one shared set of 2.
    assert!(
        std::rc::Rc::ptr_eq(&a_set, &b_set),
        "TreeA and TreeB must share one RecursiveSet",
    );
    assert_eq!(a_set.len(), 2, "the SCC seals into a set of 2 members");
    let a_idx = a_set.index_of("TreeA").unwrap();
    let b_idx = a_set.index_of("TreeB").unwrap();
    // TreeA.b is a SetLocal to TreeB's index; TreeB.a is a SetLocal to TreeA's index.
    assert_eq!(a_fields[0].0, "b");
    assert_eq!(a_fields[0].1, KType::SetLocal(b_idx));
    assert_eq!(b_fields[0].0, "a");
    assert_eq!(b_fields[0].1, KType::SetLocal(a_idx));
    assert!(scope.bindings().pending_types().is_empty());
}

/// Three-way mutual recursion: A → B → C → A. One shared set of 3; each field is a
/// `SetLocal` to the next member.
#[test]
fn three_way_mutual_recursion_struct_chain() {
    use crate::machine::execute::Scheduler;
    use crate::parse::parse;
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    let mut sched = Scheduler::new();
    for e in parse("STRUCT Aaa = (b :Bbb)\nSTRUCT Bbb = (c :Ccc)\nSTRUCT Ccc = (a :Aaa)").unwrap() {
        sched.add_dispatch(e, scope);
    }
    sched.execute().unwrap();
    let (set, _) = struct_set_and_fields(scope, "Aaa");
    assert_eq!(set.len(), 3, "the 3-way SCC seals into a set of 3");
    for (from, expected_field, expected_target) in [
        ("Aaa", "b", "Bbb"),
        ("Bbb", "c", "Ccc"),
        ("Ccc", "a", "Aaa"),
    ] {
        let (from_set, fields) = struct_set_and_fields(scope, from);
        assert!(
            std::rc::Rc::ptr_eq(&from_set, &set),
            "{from} must share the one SCC set",
        );
        let target_idx = set.index_of(expected_target).unwrap();
        assert_eq!(fields[0].0, expected_field);
        assert_eq!(
            fields[0].1,
            KType::SetLocal(target_idx),
            "{from}.{expected_field} must seal to SetLocal({expected_target})",
        );
    }
}
