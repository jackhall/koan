//! `tagged` interpret/execute integration tests.

use std::cell::RefCell;
use std::rc::Rc;

use super::*;

use super::run;

#[test]
fn tagged_union_full_program_via_type_token() {
    let arena = RuntimeArena::new();
    let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    run(
        "UNION Outcome = (Ok :Str Err :Str)\n\
         LET r = (Outcome (Ok \"all good\"))\n\
         MATCH (r) -> :Str WITH (Ok -> (PRINT it) Err -> (PRINT \"failed\"))",
        &arena,
        captured.clone(),
    );
    assert_eq!(captured.borrow().as_slice(), b"all good\n");
}

#[test]
fn tagged_union_full_program_constructs_and_matches() {
    let arena = RuntimeArena::new();
    let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    run(
        "UNION Outcome = (Ok :Str Err :Str)\n\
         LET r = (Outcome (Err \"oops\"))\n\
         MATCH (r) -> :Str WITH (Ok -> (PRINT \"good\") Err -> (PRINT it))",
        &arena,
        captured.clone(),
    );
    assert_eq!(captured.borrow().as_slice(), b"oops\n");
}

#[test]
fn tagged_union_none_branch_runs() {
    let arena = RuntimeArena::new();
    let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    run(
        "UNION Maybe = (Some :Number None :Null)\n\
         LET m = (Maybe (None null))\n\
         MATCH (m) -> :Str WITH (Some -> (PRINT \"some-branch\") None -> (PRINT \"none-branch\"))",
        &arena,
        captured.clone(),
    );
    assert_eq!(captured.borrow().as_slice(), b"none-branch\n");
}

/// Each variant is its own dispatchable type: two overloads keyed on `:(Maybe Some)` and
/// `:(Maybe None)` select by the value's variant identity, the criterion-1/3 headline.
#[test]
fn variant_typed_overloads_dispatch_by_variant() {
    let arena = RuntimeArena::new();
    let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    run(
        "UNION Maybe = (Some :Number None :Null)\n\
         FN (DESC x :(Maybe Some)) -> :Str = (\"is-some\")\n\
         FN (DESC x :(Maybe None)) -> :Str = (\"is-none\")\n\
         PRINT (DESC (Maybe (Some 1)))\n\
         PRINT (DESC (Maybe (None null)))",
        &arena,
        captured.clone(),
    );
    assert_eq!(captured.borrow().as_slice(), b"is-some\nis-none\n");
}

/// A single-variant slot rejects the wrong variant: a function accepting only `Some`
/// has no overload admitting a `None`, so the call fails dispatch.
#[test]
fn variant_typed_slot_rejects_other_variant() {
    use crate::machine::execute::interpret::interpret_with_writer;
    use crate::machine::KErrorKind;
    let result = interpret_with_writer(
        "UNION Maybe = (Some :Number None :Null)\n\
         FN (ONLYSOME x :(Maybe Some)) -> :Str = (\"ok\")\n\
         ONLYSOME (Maybe (None null))",
        Box::new(std::io::sink()),
    );
    match result {
        Err(e) => assert!(
            matches!(&e.kind, KErrorKind::DispatchFailed { .. }),
            "expected DispatchFailed when a None reaches a Some-only slot, got {e}",
        ),
        Ok(()) => panic!("expected dispatch failure for None into a :(Maybe Some) slot"),
    }
}

/// The union type still admits every variant: a `:Maybe` slot accepts a `None` value
/// even though that value's `ktype()` is now the `None` variant refinement.
#[test]
fn union_typed_slot_admits_any_variant() {
    let arena = RuntimeArena::new();
    let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    run(
        "UNION Maybe = (Some :Number None :Null)\n\
         FN (ANY x :Maybe) -> :Str = (\"any-variant\")\n\
         PRINT (ANY (Maybe (None null)))",
        &arena,
        captured.clone(),
    );
    assert_eq!(captured.borrow().as_slice(), b"any-variant\n");
}

/// `:(Maybe Some)` is a first-class type value reached through its union; it renders
/// back to its union-qualified surface.
#[test]
fn variant_type_value_renders_union_qualified() {
    let arena = RuntimeArena::new();
    let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    run(
        "UNION Maybe = (Some :Number None :Null)\n\
         PRINT :(Maybe Some)",
        &arena,
        captured.clone(),
    );
    assert_eq!(captured.borrow().as_slice(), b":(Maybe Some)\n");
}

/// A bare token that is not a variant of the union is rejected at the variant-reference
/// surface, listing the real variants.
#[test]
fn unknown_variant_reference_errors() {
    use crate::machine::execute::interpret::interpret_with_writer;
    use crate::machine::KErrorKind;
    let result = interpret_with_writer(
        "UNION Maybe = (Some :Number None :Null)\n\
         PRINT :(Maybe Bogus)",
        Box::new(std::io::sink()),
    );
    match result {
        Err(e) => assert!(
            matches!(&e.kind, KErrorKind::ShapeError(msg)
                if msg.contains("Bogus") && msg.contains("not a variant")),
            "expected a 'not a variant' ShapeError, got {e}",
        ),
        Ok(()) => panic!("expected error for an unknown variant reference"),
    }
}
