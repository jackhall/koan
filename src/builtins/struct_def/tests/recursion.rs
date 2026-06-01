//! Self-recursive and mutually-recursive struct elaboration.

use std::rc::Rc;

use crate::builtins::test_support::{parse_one, run_one, run_root_silent};
use crate::machine::model::types::UserTypeKind;
use crate::machine::model::KType;
use crate::machine::{RuntimeArena, Scope};

/// Read a STRUCT's field schema off its type-side identity (`bindings.types[name]`).
/// STRUCT is type-only now — the fields ride the `UserType { Struct { fields } }`
/// payload, not a value-side carrier. Pins the "fresh `types[name]` lookup returns the
/// full schema, not the cycle-close empty pre-install" invariant.
fn struct_fields<'a>(scope: &'a Scope<'a>, name: &str) -> Rc<Vec<(String, KType<'a>)>> {
    match scope.resolve_type(name) {
        Some(KType::UserType { kind: UserTypeKind::Struct { fields }, .. }) => Rc::clone(fields),
        other => panic!("expected {name} to be a Struct identity in types, got {other:?}"),
    }
}

/// Self-recursive STRUCT, positional sigil: `STRUCT Tree = (children :(List Tree))`
/// elaborates the field as `List(RecursiveRef("Tree"))` inline through the threaded
/// elaborator (`try_synth_legacy`), without ever leaving the body's SCC context.
#[test]
fn recursive_struct_tree_elaborates_with_recursive_ref_on_field() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run_one(scope, parse_one("STRUCT Tree = (children :(List Tree))"));
    let fields = struct_fields(scope, "Tree");
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].0, "children");
    assert_eq!(
        fields[0].1,
        KType::List(Box::new(KType::RecursiveRef("Tree".into()))),
    );
}

/// Self-recursive STRUCT, keyworded sigil: `STRUCT Tree = (children :(LIST OF Tree))`.
/// The keyworded sigil sub-Dispatches through the standalone dispatcher, which carries
/// no SCC context; `rewrite_threaded_self_refs` pre-resolves the `Tree` self-reference
/// to a `RecursiveRef` carrier before the sub-Dispatch, so the field lowers to
/// `List(RecursiveRef("Tree"))` rather than closing a scheduler-deadlock cycle on
/// `Tree`'s own placeholder.
#[test]
fn recursive_struct_tree_keyworded_list_of_lowers_to_recursive_ref() {
    let arena = RuntimeArena::new();
    let scope = run_root_silent(&arena);
    run_one(scope, parse_one("STRUCT Tree = (children :(LIST OF Tree))"));
    let fields = struct_fields(scope, "Tree");
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].0, "children");
    assert_eq!(
        fields[0].1,
        KType::List(Box::new(KType::RecursiveRef("Tree".into()))),
    );
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
    let b_fields = struct_fields(scope, "Bb");
    assert_eq!(b_fields[0].0, "y");
    assert!(
        !matches!(b_fields[0].1, KType::RecursiveRef(_)),
        "Bb's `y` field must not be wrapped in RecursiveRef, got {:?}",
        b_fields[0].1,
    );
}

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
    let a_fields = struct_fields(scope, "TreeA");
    let b_fields = struct_fields(scope, "TreeB");
    // Cross-member references resolve to `UserType` (not `RecursiveRef`) because
    // SCC cycle-close pre-installs both identities before either finalizes; finalize
    // then upserts each schema-bearing identity over the empty pre-install.
    assert_eq!(a_fields[0].0, "b");
    assert!(
        matches!(&a_fields[0].1, KType::UserType { kind: UserTypeKind::Struct { .. }, name, .. } if name == "TreeB"),
        "TreeA.b expected UserType{{TreeB}}, got {:?}",
        a_fields[0].1,
    );
    assert_eq!(b_fields[0].0, "a");
    assert!(
        matches!(&b_fields[0].1, KType::UserType { kind: UserTypeKind::Struct { .. }, name, .. } if name == "TreeA"),
        "TreeB.a expected UserType{{TreeA}}, got {:?}",
        b_fields[0].1,
    );
    assert!(scope.bindings().pending_types().is_empty());
}

/// Three-way mutual recursion: A → B → C → A. Exercises SCC DFS past depth 2.
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
    for (from, expected_field, expected_target) in
        [("Aaa", "b", "Bbb"), ("Bbb", "c", "Ccc"), ("Ccc", "a", "Aaa")]
    {
        let fields = struct_fields(scope, from);
        assert_eq!(fields[0].0, expected_field);
        assert!(
            matches!(&fields[0].1, KType::UserType { kind: UserTypeKind::Struct { .. }, name, .. } if name == expected_target),
            "{from}.{expected_field} expected UserType{{{expected_target}}}, got {:?}",
            fields[0].1,
        );
    }
}
