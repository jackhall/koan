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
    assert!(matches!(data.get("x").map(|(o, _)| *o), Some(KObject::Number(n)) if *n == 42.0));
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
    assert!(matches!(data.get("msg").map(|(o, _)| *o), Some(KObject::KString(s)) if *s == "hello world!"));
}

#[test]
fn let_binds_a_list_literal_of_numbers() {
    let arena = RuntimeArena::new();
    let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    let scope = run("LET xs = [1 2 3]\n", &arena, captured);
    let data = scope.bindings().data();
    match data.get("xs").map(|(o, _)| *o) {
        Some(KObject::List(items, _)) => {
            assert_eq!(items.len(), 3);
            assert!(matches!(items[0], KObject::Number(n) if n == 1.0));
            assert!(matches!(items[2], KObject::Number(n) if n == 3.0));
        }
        _ => panic!("expected `xs` bound to a List"),
    }
}

/// A *stamped* empty list (via a typed FN return) is NOT flagged by the empty-container
/// rule: its carrier carries a concrete element type, so binding the FN's result through
/// an untyped `LET` succeeds and the binding records the stamped `List<Number>`.
#[test]
fn let_binds_stamped_empty_list_from_typed_fn_return() {
    let arena = RuntimeArena::new();
    let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    let scope = run(
        "FN (EMPTY) -> :(List Number) = ([])\nLET xs = (EMPTY)\n",
        &arena,
        captured,
    );
    let data = scope.bindings().data();
    match data.get("xs").map(|(o, _)| *o) {
        Some(obj @ KObject::List(items, _)) => {
            assert!(items.is_empty());
            assert_eq!(
                obj.ktype(),
                crate::machine::model::types::KType::List(Box::new(
                    crate::machine::model::types::KType::Number,
                )),
            );
        }
        _ => panic!("expected `xs` bound to a stamped empty List<Number>"),
    }
}

/// Empty-container error rule: binding a bare `[]` through an untyped `LET` is an error —
/// an empty list has no element type to infer and was never stamped by an annotation
/// upstream. The user must annotate the producing boundary or use a non-empty literal.
#[test]
fn let_binds_an_empty_list_literal_errors() {
    use crate::machine::KErrorKind;
    use crate::machine::execute::interpret_with_writer;
    let result = interpret_with_writer("LET xs = []\n", Box::new(std::io::sink()));
    match result {
        Err(e) => assert!(
            matches!(&e.kind, KErrorKind::ShapeError(msg) if msg.contains("empty container")),
            "expected empty-container ShapeError, got {e}",
        ),
        Ok(()) => panic!("expected empty-container error binding `[]`"),
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
    match data.get("xs").map(|(o, _)| *o) {
        Some(KObject::List(items, _)) => {
            assert_eq!(items.len(), 3);
            assert!(matches!(items[0], KObject::Number(n) if n == 1.0));
            assert!(matches!(items[1], KObject::Number(n) if n == 7.0));
            assert!(matches!(items[2], KObject::Number(n) if n == 3.0));
        }
        _ => panic!("expected `xs` bound to a List"),
    }
    assert!(matches!(data.get("y").map(|(o, _)| *o), Some(KObject::Number(n)) if *n == 7.0));
}

#[test]
fn multiline_list_literal_binds_correctly() {
    let arena = RuntimeArena::new();
    let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    let scope = run("LET xs = [\n  1\n  2\n  3\n]\n", &arena, captured);
    let data = scope.bindings().data();
    match data.get("xs").map(|(o, _)| *o) {
        Some(KObject::List(items, _)) => {
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
    match data.get("xs").map(|(o, _)| *o) {
        Some(KObject::List(outer, _)) => {
            assert_eq!(outer.len(), 2);
            match &outer[0] {
                KObject::List(inner, _) => {
                    assert_eq!(inner.len(), 2);
                    assert!(matches!(inner[0], KObject::Number(n) if n == 1.0));
                }
                _ => panic!("inner[0] should be a List"),
            }
        }
        _ => panic!("expected `xs` bound to a List"),
    }
}
