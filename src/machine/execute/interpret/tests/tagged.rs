//! `tagged` interpret/execute integration tests.

use std::cell::RefCell;
use std::rc::Rc;

use super::*;

use super::run;
use crate::machine::program_storage;

#[test]
fn tagged_union_full_program_via_type_token() {
    let program = program_storage();
    let region = run_root_storage();
    let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    run(
        &program,
        "UNION Outcome = (Ok :Str Err :Str)\n\
         LET r = (Outcome.Ok \"all good\")\n\
         MATCH (r) OVER Outcome -> :Str WITH (Ok -> (PRINT it) Err -> (PRINT \"failed\"))",
        &region,
        captured.clone(),
    );
    assert_eq!(captured.borrow().as_slice(), b"all good\n");
}

#[test]
fn tagged_union_full_program_constructs_and_matches() {
    let program = program_storage();
    let region = run_root_storage();
    let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    run(
        &program,
        "UNION Outcome = (Ok :Str Err :Str)\n\
         LET r = (Outcome.Err \"oops\")\n\
         MATCH (r) OVER Outcome -> :Str WITH (Ok -> (PRINT \"good\") Err -> (PRINT it))",
        &region,
        captured.clone(),
    );
    assert_eq!(captured.borrow().as_slice(), b"oops\n");
}

#[test]
fn tagged_union_none_branch_runs() {
    let program = program_storage();
    let region = run_root_storage();
    let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    run(
        &program,
        "UNION Maybe = (Some :Number None :Null)\n\
         LET m = (Maybe.None null)\n\
         MATCH (m) OVER Maybe -> :Str WITH (Some -> (PRINT \"some-branch\") None -> (PRINT \"none-branch\"))",
        &region,
        captured.clone(),
    );
    assert_eq!(captured.borrow().as_slice(), b"none-branch\n");
}

/// Each variant is its own dispatchable type: two overloads keyed on `:(Maybe.Some)` and
/// `:(Maybe.None)` select by the value's variant identity, the criterion-1/3 headline.
#[test]
fn variant_typed_overloads_dispatch_by_variant() {
    let program = program_storage();
    let region = run_root_storage();
    let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    run(
        &program,
        "UNION Maybe = (Some :Number None :Null)\n\
         FN (DESC x :(Maybe.Some)) -> :Str = (\"is-some\")\n\
         FN (DESC x :(Maybe.None)) -> :Str = (\"is-none\")\n\
         PRINT (DESC (Maybe.Some 1))\n\
         PRINT (DESC (Maybe.None null))",
        &region,
        captured.clone(),
    );
    assert_eq!(captured.borrow().as_slice(), b"is-some\nis-none\n");
}

/// A single-variant slot rejects the wrong variant: a function accepting only `Some`
/// has no overload admitting a `None`, so the call fails dispatch.
#[test]
fn variant_typed_slot_rejects_other_variant() {
    use crate::machine::KErrorKind;
    use crate::machine::execute::interpret_with_writer;
    let result = interpret_with_writer(
        "UNION Maybe = (Some :Number None :Null)\n\
         FN (ONLYSOME x :(Maybe.Some)) -> :Str = (\"ok\")\n\
         ONLYSOME (Maybe.None null)",
        Box::new(std::io::sink()),
    );
    match result {
        Err(e) => assert!(
            matches!(&e.kind, KErrorKind::DispatchFailed { .. }),
            "expected DispatchFailed when a None reaches a Some-only slot, got {e}",
        ),
        Ok(()) => panic!("expected dispatch failure for None into a :(Maybe.Some) slot"),
    }
}

/// The union type still admits every variant: a `:Maybe` slot accepts a `None` value
/// even though that value's `ktype()` is now the `None` variant refinement.
#[test]
fn union_typed_slot_admits_any_variant() {
    let program = program_storage();
    let region = run_root_storage();
    let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    run(
        &program,
        "UNION Maybe = (Some :Number None :Null)\n\
         FN (ANY x :Maybe) -> :Str = (\"any-variant\")\n\
         PRINT (ANY (Maybe.None null))",
        &region,
        captured.clone(),
    );
    assert_eq!(captured.borrow().as_slice(), b"any-variant\n");
}

/// `:(Maybe.Some)` is a first-class type value reached through its union — the `Some` variant's
/// per-tag newtype `SetMember`, which renders by its own member name.
#[test]
fn variant_type_value_renders_member_name() {
    let program = program_storage();
    let region = run_root_storage();
    let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    run(
        &program,
        "UNION Maybe = (Some :Number None :Null)\n\
         PRINT :(Maybe.Some)",
        &region,
        captured.clone(),
    );
    assert_eq!(captured.borrow().as_slice(), b"Some\n");
}

/// A name that is not a member of the union is rejected at the projection surface, listing the
/// real members.
#[test]
fn unknown_variant_reference_errors() {
    use crate::machine::KErrorKind;
    use crate::machine::execute::interpret_with_writer;
    let result = interpret_with_writer(
        "UNION Maybe = (Some :Number None :Null)\n\
         PRINT :(Maybe.Bogus)",
        Box::new(std::io::sink()),
    );
    match result {
        Err(e) => assert!(
            matches!(&e.kind, KErrorKind::ShapeError(msg)
                if msg.contains("Bogus") && msg.contains("not a member")),
            "expected a 'not a member' ShapeError, got {e}",
        ),
        Ok(()) => panic!("expected error for an unknown variant reference"),
    }
}

/// A union is not itself callable, whatever its members are: applying the union name is a shape
/// error naming the projection surface and listing the members.
#[test]
fn a_union_refuses_direct_application() {
    use crate::machine::KErrorKind;
    use crate::machine::execute::interpret_with_writer;
    let result = interpret_with_writer(
        "NEWTYPE (Type AS Boxed)\n\
         NEWTYPE Wrapped = :Number\n\
         LET Un = :(Boxed | Wrapped)\n\
         PRINT (Un (Boxed \"x\"))",
        Box::new(std::io::sink()),
    );
    match result {
        Err(e) => assert!(
            matches!(&e.kind, KErrorKind::ShapeError(msg)
                if msg.contains("a union has no direct application")
                    && msg.contains("member projection")),
            "expected the no-direct-application ShapeError, got {e}",
        ),
        Ok(()) => panic!("expected error applying a union name"),
    }
}

/// Construction is ordinary application of the projected member: `Maybe.Some 42` evaluates the
/// ATTR head to the variant's newtype identity and wraps the trailing value through the same door
/// a standalone `NEWTYPE` takes. There is no union-application form behind it.
#[test]
fn projected_member_constructs_by_application() {
    let program = program_storage();
    let region = run_root_storage();
    let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    run(
        &program,
        "UNION Maybe = (Some :Number None :Null)\n\
         PRINT (Maybe.Some 42)",
        &region,
        captured.clone(),
    );
    assert_eq!(captured.borrow().as_slice(), b"Some(42)\n");
}

/// A recursive union keeps its layers: the payload's identity differs from the member's repr, so
/// the peel-or-hold rule holds the nested variant rather than collapsing it.
#[test]
fn projected_member_construction_keeps_nested_layers() {
    let program = program_storage();
    let region = run_root_storage();
    let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    run(
        &program,
        "UNION Nat = (Zero :Null Succ :Nat)\n\
         PRINT (Nat.Succ (Nat.Zero null))",
        &region,
        captured.clone(),
    );
    assert_eq!(captured.borrow().as_slice(), b"Succ(Zero(null))\n");
}

/// A record-repr variant takes the record body the same way a record-repr `NEWTYPE` does.
#[test]
fn projected_member_constructs_a_record_repr_variant() {
    let program = program_storage();
    let region = run_root_storage();
    let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    run(
        &program,
        "UNION Shape = (Circle :{r :Number} Square :{s :Number})\n\
         PRINT (Shape.Circle {r = 2})",
        &region,
        captured.clone(),
    );
    assert_eq!(captured.borrow().as_slice(), b"Circle({r = 2})\n");
}

/// The member's declared payload type is the construction's contract: a mismatched payload is a
/// type error at the wrap, not a silently retyped value.
#[test]
fn projected_member_construction_checks_the_payload_type() {
    use crate::machine::KErrorKind;
    use crate::machine::execute::interpret_with_writer;
    let result = interpret_with_writer(
        "UNION Maybe = (Some :Number None :Null)\n\
         LET m = (Maybe.Some \"x\")",
        Box::new(std::io::sink()),
    );
    match result {
        Err(e) => assert!(
            matches!(&e.kind, KErrorKind::TypeMismatch { .. }),
            "expected a payload TypeMismatch, got {e}",
        ),
        Ok(()) => panic!("expected a payload type error"),
    }
}

/// A bare projection with no body is the member's *type value*, not a construction — the two are
/// told apart by whether the application has trailing parts at all.
#[test]
fn bare_projection_is_a_type_value_not_a_construction() {
    let program = program_storage();
    let region = run_root_storage();
    let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    run(
        &program,
        "UNION Maybe = (Some :Number None :Null)\n\
         PRINT (Maybe.Some)",
        &region,
        captured.clone(),
    );
    assert_eq!(captured.borrow().as_slice(), b"Some\n");
}

/// `:(Maybe.Some)` in annotation position is strictly more specific than `:Maybe`: the member
/// refines the union, so the variant overload wins the dispatch tournament outright.
#[test]
fn a_variant_annotation_outranks_its_union() {
    let program = program_storage();
    let region = run_root_storage();
    let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    run(
        &program,
        "UNION Maybe = (Some :Number None :Null)\n\
         FN (DESC x :Maybe) -> :Str = (\"any\")\n\
         FN (DESC x :(Maybe.Some)) -> :Str = (\"just-some\")\n\
         PRINT (DESC (Maybe.Some 1))\n\
         PRINT (DESC (Maybe.None null))",
        &region,
        captured.clone(),
    );
    assert_eq!(captured.borrow().as_slice(), b"just-some\nany\n");
}

/// A union schema field names a sibling variant of a union still under seal through the same
/// projection: `:(Tree.Leaf)` lowers against the declaration window rather than sub-dispatching,
/// which would deadlock on this very seal's producer.
#[test]
fn a_pre_seal_sibling_variant_is_named_by_projection() {
    let program = program_storage();
    let region = run_root_storage();
    let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    run(
        &program,
        "UNION Tree = (Leaf :Number Node :(Tree.Leaf))\n\
         PRINT (Tree.Node (Tree.Leaf 1))",
        &region,
        captured.clone(),
    );
    // `Node`'s declared repr *is* `Leaf`, so the newtype collapse folds the redundant layer: the
    // sibling reference resolved, which is what this pins.
    assert_eq!(captured.borrow().as_slice(), b"Node(1)\n");
}

/// The juxtaposed spelling names no sibling variant in the pre-seal position either: `:(Tree Leaf)`
/// falls through to ordinary elaboration and errors there.
#[test]
fn the_juxtaposed_pre_seal_sibling_spelling_is_retired() {
    use crate::machine::execute::interpret_with_writer;
    let result = interpret_with_writer(
        "UNION Tree = (Leaf :Number Node :(Tree Leaf))\n\
         PRINT (Tree.Leaf 1)",
        Box::new(std::io::sink()),
    );
    assert!(
        result.is_err(),
        "expected the juxtaposed sibling spelling to no longer resolve",
    );
}
