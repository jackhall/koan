//! `dict` interpret/execute integration tests.

use std::cell::RefCell;
use std::rc::Rc;

use super::*;
use crate::machine::model::KObject;
use crate::machine::model::types::Serializable;
use crate::machine::model::values::KKey;
use crate::machine::KErrorKind;

use super::run;

fn lookup_string_key<'a, 'b>(
    d: &'b std::collections::HashMap<Box<dyn Serializable + 'a>, KObject<'a>>,
    key: &str,
) -> Option<&'b KObject<'a>> {
    let probe: Box<dyn Serializable> = Box::new(KKey::String(key.to_string()));
    d.get(&probe)
}

fn lookup_number_key<'a, 'b>(
    d: &'b std::collections::HashMap<Box<dyn Serializable + 'a>, KObject<'a>>,
    key: f64,
) -> Option<&'b KObject<'a>> {
    let probe: Box<dyn Serializable> = Box::new(KKey::Number(key));
    d.get(&probe)
}

fn lookup_bool_key<'a, 'b>(
    d: &'b std::collections::HashMap<Box<dyn Serializable + 'a>, KObject<'a>>,
    key: bool,
) -> Option<&'b KObject<'a>> {
    let probe: Box<dyn Serializable> = Box::new(KKey::Bool(key));
    d.get(&probe)
}

#[test]
fn let_binds_an_empty_dict_literal() {
    let arena = RuntimeArena::new();
    let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    let scope = run("LET d = {}\n", &arena, captured);
    let data = scope.bindings().data();
    match data.get("d") {
        Some(KObject::Dict(entries)) => assert!(entries.is_empty()),
        _ => panic!("expected `d` bound to an empty Dict"),
    }
}

#[test]
fn let_binds_a_dict_with_string_keys() {
    let arena = RuntimeArena::new();
    let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    let scope = run(r#"LET d = {"a": 1, "b": 2}"#, &arena, captured);
    let data = scope.bindings().data();
    match data.get("d") {
        Some(KObject::Dict(entries)) => {
            assert_eq!(entries.len(), 2);
            assert!(matches!(lookup_string_key(entries, "a"), Some(KObject::Number(n)) if *n == 1.0));
            assert!(matches!(lookup_string_key(entries, "b"), Some(KObject::Number(n)) if *n == 2.0));
        }
        _ => panic!("expected `d` bound to a Dict"),
    }
}

#[test]
fn let_binds_a_dict_with_number_keys() {
    let arena = RuntimeArena::new();
    let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    let scope = run(r#"LET d = {1: "a", 2: "b"}"#, &arena, captured);
    let data = scope.bindings().data();
    match data.get("d") {
        Some(KObject::Dict(entries)) => {
            assert_eq!(entries.len(), 2);
            assert!(matches!(lookup_number_key(entries, 1.0), Some(KObject::KString(s)) if s == "a"));
            assert!(matches!(lookup_number_key(entries, 2.0), Some(KObject::KString(s)) if s == "b"));
        }
        _ => panic!("expected `d` bound to a Dict"),
    }
}

#[test]
fn let_binds_a_dict_with_bool_keys() {
    let arena = RuntimeArena::new();
    let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    let scope = run("LET d = {true: 1, false: 0}\n", &arena, captured);
    let data = scope.bindings().data();
    match data.get("d") {
        Some(KObject::Dict(entries)) => {
            assert_eq!(entries.len(), 2);
            assert!(matches!(lookup_bool_key(entries, true), Some(KObject::Number(n)) if *n == 1.0));
            assert!(matches!(lookup_bool_key(entries, false), Some(KObject::Number(n)) if *n == 0.0));
        }
        _ => panic!("expected `d` bound to a Dict"),
    }
}

#[test]
fn bare_identifier_key_is_looked_up() {
    let arena = RuntimeArena::new();
    let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    let scope = run(
        "LET name = \"alice\"\nLET d = {name: 1}\n",
        &arena,
        captured,
    );
    let data = scope.bindings().data();
    match data.get("d") {
        Some(KObject::Dict(entries)) => {
            assert_eq!(entries.len(), 1);
            // The key should be the looked-up value of `name`, not the literal "name".
            assert!(matches!(lookup_string_key(entries, "alice"), Some(KObject::Number(n)) if *n == 1.0));
            assert!(lookup_string_key(entries, "name").is_none());
        }
        _ => panic!("expected `d` bound to a Dict"),
    }
}

#[test]
fn sub_expression_as_value_evaluates_eagerly() {
    let arena = RuntimeArena::new();
    let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    let scope = run(r#"LET d = {"a": (LET y = 7)}"#, &arena, captured);
    let data = scope.bindings().data();
    match data.get("d") {
        Some(KObject::Dict(entries)) => {
            assert!(matches!(lookup_string_key(entries, "a"), Some(KObject::Number(n)) if *n == 7.0));
        }
        _ => panic!("expected `d` bound to a Dict"),
    }
    assert!(matches!(data.get("y"), Some(KObject::Number(n)) if *n == 7.0));
}

#[test]
fn sub_expression_as_key_evaluates() {
    let arena = RuntimeArena::new();
    let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    let scope = run(
        "LET k = \"x\"\nLET d = {(k): 1}\n",
        &arena,
        captured,
    );
    let data = scope.bindings().data();
    match data.get("d") {
        Some(KObject::Dict(entries)) => {
            assert!(matches!(lookup_string_key(entries, "x"), Some(KObject::Number(n)) if *n == 1.0));
        }
        _ => panic!("expected `d` bound to a Dict"),
    }
}

#[test]
fn multiline_dict_binds_correctly() {
    let arena = RuntimeArena::new();
    let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    let scope = run(
        "LET d = {\n  \"a\": 1\n  \"b\": 2\n}\n",
        &arena,
        captured,
    );
    let data = scope.bindings().data();
    match data.get("d") {
        Some(KObject::Dict(entries)) => {
            assert_eq!(entries.len(), 2);
            assert!(matches!(lookup_string_key(entries, "a"), Some(KObject::Number(n)) if *n == 1.0));
            assert!(matches!(lookup_string_key(entries, "b"), Some(KObject::Number(n)) if *n == 2.0));
        }
        _ => panic!("expected `d` bound to a Dict"),
    }
}

#[test]
fn nested_dict_in_list_binds_correctly() {
    let arena = RuntimeArena::new();
    let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    let scope = run(r#"LET xs = [{"a": 1} {"b": 2}]"#, &arena, captured);
    let data = scope.bindings().data();
    match data.get("xs") {
        Some(KObject::List(outer)) => {
            assert_eq!(outer.len(), 2);
            match &outer[0] {
                KObject::Dict(d) => assert!(matches!(
                    lookup_string_key(d, "a"),
                    Some(KObject::Number(n)) if *n == 1.0,
                )),
                _ => panic!("outer[0] should be a Dict"),
            }
        }
        _ => panic!("expected `xs` bound to a List"),
    }
}

#[test]
fn non_scalar_key_returns_shape_error() {
    // Bind a variable to a list, then use it as a dict key via lookup. The list reaches
    // `KKey::try_from_kobject` at materialization time and is rejected.
    let result = interpret_with_writer(
        "LET k = [1 2]\nLET d = {(k): 1}",
        Box::new(std::io::sink()),
    );
    match result {
        Err(e) => assert!(
            matches!(&e.kind, KErrorKind::ShapeError(msg) if msg.contains("dict key")),
            "expected ShapeError mentioning dict key, got {e}",
        ),
        Ok(()) => panic!("expected ShapeError for non-scalar dict key"),
    }
}

#[test]
fn unbound_identifier_key_returns_unbound_name() {
    let result = interpret_with_writer("LET d = {missing: 1}", Box::new(std::io::sink()));
    match result {
        Err(e) => assert!(
            matches!(&e.kind, KErrorKind::UnboundName(name) if name == "missing"),
            "expected UnboundName(\"missing\"), got {e}",
        ),
        Ok(()) => panic!("expected UnboundName for missing identifier key"),
    }
}
