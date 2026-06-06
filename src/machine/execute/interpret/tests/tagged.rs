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
        "UNION Outcome = (ok :Str err :Str)\n\
         LET r = (Outcome (ok \"all good\"))\n\
         MATCH (r) -> :Str WITH (ok -> (PRINT it) err -> (PRINT \"failed\"))",
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
        "UNION Outcome = (ok :Str err :Str)\n\
         LET r = (Outcome (err \"oops\"))\n\
         MATCH (r) -> :Str WITH (ok -> (PRINT \"good\") err -> (PRINT it))",
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
        "UNION Maybe = (some :Number none :Null)\n\
         LET m = (Maybe (none null))\n\
         MATCH (m) -> :Str WITH (some -> (PRINT \"some-branch\") none -> (PRINT \"none-branch\"))",
        &arena,
        captured.clone(),
    );
    assert_eq!(captured.borrow().as_slice(), b"none-branch\n");
}
