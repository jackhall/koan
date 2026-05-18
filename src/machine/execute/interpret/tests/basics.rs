//! `basics` interpret/execute integration tests.

use std::cell::RefCell;
use std::rc::Rc;

use super::*;
use crate::machine::model::KObject;

use super::run;

#[test]
fn interprets_let_and_print() {
    let arena = RuntimeArena::new();
    let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    let scope = run("LET x = 42\nPRINT \"hello\"\n", &arena, captured.clone());

    assert_eq!(captured.borrow().as_slice(), b"hello\n");
    let data = scope.bindings().data();
    assert!(matches!(data.get("x"), Some(KObject::Number(n)) if *n == 42.0));
}

#[test]
fn interprets_match_via_print() {
    let arena = RuntimeArena::new();
    let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    run(
        r#"PRINT (MATCH true WITH (true -> ("yes") false -> ("no")))"#,
        &arena,
        captured.clone(),
    );
    assert_eq!(captured.borrow().as_slice(), b"yes\n");
}

#[test]
fn match_branch_resolves_outer_name() {
    // The branch body's lazy slot evaluates in the surrounding scope, so a name bound
    // before the MATCH (`greeting`) resolves through the outer chain at branch-dispatch
    // time. Integration-level coverage of the lazy-slot/closure-capture machinery from
    // a koan program (the `match_case` unit tests exercise it via test scaffolding).
    let arena = RuntimeArena::new();
    let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    run(
        "LET greeting = \"hi\"\nPRINT (MATCH true WITH (true -> (greeting) false -> (\"no\")))\n",
        &arena,
        captured.clone(),
    );
    assert_eq!(captured.borrow().as_slice(), b"hi\n");
}

#[test]
fn match_unmatched_branch_skips_let_side_effect() {
    // The unmatched branch's body is never dispatched, so its `LET y = 1` must not
    // execute and `y` must remain unbound. Verifies the lazy-slot guarantee end-to-end:
    // unmatched branches are inert.
    let arena = RuntimeArena::new();
    let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    let scope = run(
        "MATCH false WITH (true -> (LET y = 1) false -> (null))\nPRINT \"after\"\n",
        &arena,
        captured.clone(),
    );
    assert!(scope.bindings().data().get("y").is_none(), "unmatched branch's LET must not have bound y");
    assert_eq!(captured.borrow().as_slice(), b"after\n");
}

#[test]
fn interprets_nested_expression() {
    let arena = RuntimeArena::new();
    let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    let scope = run(r#"(PRINT (LET msg = "hello world!"))"#, &arena, captured.clone());

    assert_eq!(captured.borrow().as_slice(), b"hello world!\n");
    let data = scope.bindings().data();
    assert!(matches!(data.get("msg"), Some(KObject::KString(s)) if *s == "hello world!"));
}

#[test]
fn let_binds_a_list_literal_of_numbers() {
    let arena = RuntimeArena::new();
    let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    let scope = run("LET xs = [1 2 3]\n", &arena, captured);
    let data = scope.bindings().data();
    match data.get("xs") {
        Some(KObject::List(items)) => {
            assert_eq!(items.len(), 3);
            assert!(matches!(items[0], KObject::Number(n) if n == 1.0));
            assert!(matches!(items[2], KObject::Number(n) if n == 3.0));
        }
        _ => panic!("expected `xs` bound to a List"),
    }
}

#[test]
fn let_binds_an_empty_list_literal() {
    let arena = RuntimeArena::new();
    let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    let scope = run("LET xs = []\n", &arena, captured);
    let data = scope.bindings().data();
    match data.get("xs") {
        Some(KObject::List(items)) => assert!(items.is_empty()),
        _ => panic!("expected `xs` bound to an empty List"),
    }
}

#[test]
fn list_literal_with_subexpression_element_evaluates_eagerly() {
    // `(LET y = 7)` evaluates as part of the list construction; afterwards `y` is bound
    // and the list contains the LET's return value (the bound number).
    let arena = RuntimeArena::new();
    let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    let scope = run("LET xs = [1 (LET y = 7) 3]\n", &arena, captured);
    let data = scope.bindings().data();
    match data.get("xs") {
        Some(KObject::List(items)) => {
            assert_eq!(items.len(), 3);
            assert!(matches!(items[0], KObject::Number(n) if n == 1.0));
            assert!(matches!(items[1], KObject::Number(n) if n == 7.0));
            assert!(matches!(items[2], KObject::Number(n) if n == 3.0));
        }
        _ => panic!("expected `xs` bound to a List"),
    }
    assert!(matches!(data.get("y"), Some(KObject::Number(n)) if *n == 7.0));
}

#[test]
fn multiline_list_literal_binds_correctly() {
    let arena = RuntimeArena::new();
    let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    let scope = run("LET xs = [\n  1\n  2\n  3\n]\n", &arena, captured);
    let data = scope.bindings().data();
    match data.get("xs") {
        Some(KObject::List(items)) => {
            assert_eq!(items.len(), 3);
            assert!(matches!(items[0], KObject::Number(n) if n == 1.0));
            assert!(matches!(items[2], KObject::Number(n) if n == 3.0));
        }
        _ => panic!("expected `xs` bound to a List"),
    }
}

#[test]
fn nested_list_literal_produces_list_of_lists() {
    let arena = RuntimeArena::new();
    let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    let scope = run("LET xs = [[1 2] [3 4]]\n", &arena, captured);
    let data = scope.bindings().data();
    match data.get("xs") {
        Some(KObject::List(outer)) => {
            assert_eq!(outer.len(), 2);
            match &outer[0] {
                KObject::List(inner) => {
                    assert_eq!(inner.len(), 2);
                    assert!(matches!(inner[0], KObject::Number(n) if n == 1.0));
                }
                _ => panic!("inner[0] should be a List"),
            }
        }
        _ => panic!("expected `xs` bound to a List"),
    }
}
