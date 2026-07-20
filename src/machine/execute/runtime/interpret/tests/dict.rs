//! `dict` interpret/execute integration tests.

use std::cell::RefCell;
use std::rc::Rc;

use super::*;
use crate::machine::model::KObject;
use crate::machine::model::{Held, KKey};
use crate::machine::KErrorKind;

use super::run;

/// Dict value cells are [`Held`]; these helpers narrow to the `Object` arm so the
/// scalar-value assertions read unchanged.
fn lookup_string_key<'run, 'b>(
    d: &'b std::collections::HashMap<KKey, Held<'run>>,
    key: &str,
) -> Option<&'b KObject<'run>> {
    d.get(&KKey::String(key.to_string()))
        .and_then(|h| h.as_object())
}

fn lookup_number_key<'run, 'b>(
    d: &'b std::collections::HashMap<KKey, Held<'run>>,
    key: f64,
) -> Option<&'b KObject<'run>> {
    d.get(&KKey::Number(key)).and_then(|h| h.as_object())
}

fn lookup_bool_key<'run, 'b>(
    d: &'b std::collections::HashMap<KKey, Held<'run>>,
    key: bool,
) -> Option<&'b KObject<'run>> {
    d.get(&KKey::Bool(key)).and_then(|h| h.as_object())
}

/// Unlike the empty-list / empty-dict rule, bare `{}` is the empty record (the top of
/// the record lattice), which is well-typed on its own — so binding it through an untyped
/// `LET` succeeds rather than tripping the empty-container rule.
#[test]
fn let_binds_an_empty_record_literal() {
    let region = run_root_storage();
    let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    let test_run = run("LET d = {}", &region, captured);
    let scope = test_run.scope;
    let data = scope.bindings().data();
    match data.get("d").map(|(o, _, _)| *o) {
        Some(KObject::Record(fields, _)) => assert!(fields.is_empty(), "expected empty record"),
        other => panic!(
            "expected `d` bound to an empty Record, got {:?}",
            other.map(|o| o.ktype().name(&test_run.types))
        ),
    }
}

#[test]
fn let_binds_a_dict_with_string_keys() {
    let region = run_root_storage();
    let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    let test_run = run(r#"LET d = {"a": 1, "b": 2}"#, &region, captured);
    let scope = test_run.scope;
    let data = scope.bindings().data();
    match data.get("d").map(|(o, _, _)| *o) {
        Some(KObject::Dict(entries, _)) => {
            assert_eq!(entries.len(), 2);
            assert!(
                matches!(lookup_string_key(entries, "a"), Some(KObject::Number(n)) if *n == 1.0)
            );
            assert!(
                matches!(lookup_string_key(entries, "b"), Some(KObject::Number(n)) if *n == 2.0)
            );
        }
        _ => panic!("expected `d` bound to a Dict"),
    }
}

#[test]
fn let_binds_a_dict_with_number_keys() {
    let region = run_root_storage();
    let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    let test_run = run(r#"LET d = {1: "a", 2: "b"}"#, &region, captured);
    let scope = test_run.scope;
    let data = scope.bindings().data();
    match data.get("d").map(|(o, _, _)| *o) {
        Some(KObject::Dict(entries, _)) => {
            assert_eq!(entries.len(), 2);
            assert!(
                matches!(lookup_number_key(entries, 1.0), Some(KObject::KString(s)) if s == "a")
            );
            assert!(
                matches!(lookup_number_key(entries, 2.0), Some(KObject::KString(s)) if s == "b")
            );
        }
        _ => panic!("expected `d` bound to a Dict"),
    }
}

#[test]
fn let_binds_a_dict_with_bool_keys() {
    let region = run_root_storage();
    let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    let test_run = run("LET d = {true: 1, false: 0}\n", &region, captured);
    let scope = test_run.scope;
    let data = scope.bindings().data();
    match data.get("d").map(|(o, _, _)| *o) {
        Some(KObject::Dict(entries, _)) => {
            assert_eq!(entries.len(), 2);
            assert!(
                matches!(lookup_bool_key(entries, true), Some(KObject::Number(n)) if *n == 1.0)
            );
            assert!(
                matches!(lookup_bool_key(entries, false), Some(KObject::Number(n)) if *n == 0.0)
            );
        }
        _ => panic!("expected `d` bound to a Dict"),
    }
}

#[test]
fn bare_identifier_key_is_looked_up() {
    let region = run_root_storage();
    let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    let test_run = run(
        "LET name = \"alice\"\nLET d = {name: 1}\n",
        &region,
        captured,
    );
    let scope = test_run.scope;
    let data = scope.bindings().data();
    match data.get("d").map(|(o, _, _)| *o) {
        Some(KObject::Dict(entries, _)) => {
            assert_eq!(entries.len(), 1);
            assert!(
                matches!(lookup_string_key(entries, "alice"), Some(KObject::Number(n)) if *n == 1.0)
            );
            assert!(lookup_string_key(entries, "name").is_none());
        }
        _ => panic!("expected `d` bound to a Dict"),
    }
}

#[test]
fn sub_expression_as_value_evaluates_eagerly() {
    let region = run_root_storage();
    let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    let test_run = run(r#"LET d = {"a": (LET y = 7)}"#, &region, captured);
    let scope = test_run.scope;
    let data = scope.bindings().data();
    match data.get("d").map(|(o, _, _)| *o) {
        Some(KObject::Dict(entries, _)) => {
            assert!(
                matches!(lookup_string_key(entries, "a"), Some(KObject::Number(n)) if *n == 7.0)
            );
        }
        _ => panic!("expected `d` bound to a Dict"),
    }
    assert!(matches!(data.get("y").map(|(o, _, _)| *o), Some(KObject::Number(n)) if *n == 7.0));
}

#[test]
fn sub_expression_as_key_evaluates() {
    let region = run_root_storage();
    let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    let test_run = run("LET k = \"x\"\nLET d = {(k): 1}\n", &region, captured);
    let scope = test_run.scope;
    let data = scope.bindings().data();
    match data.get("d").map(|(o, _, _)| *o) {
        Some(KObject::Dict(entries, _)) => {
            assert!(
                matches!(lookup_string_key(entries, "x"), Some(KObject::Number(n)) if *n == 1.0)
            );
        }
        _ => panic!("expected `d` bound to a Dict"),
    }
}

#[test]
fn multiline_dict_binds_correctly() {
    let region = run_root_storage();
    let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    let test_run = run("LET d = {\n  \"a\": 1\n  \"b\": 2\n}\n", &region, captured);
    let scope = test_run.scope;
    let data = scope.bindings().data();
    match data.get("d").map(|(o, _, _)| *o) {
        Some(KObject::Dict(entries, _)) => {
            assert_eq!(entries.len(), 2);
            assert!(
                matches!(lookup_string_key(entries, "a"), Some(KObject::Number(n)) if *n == 1.0)
            );
            assert!(
                matches!(lookup_string_key(entries, "b"), Some(KObject::Number(n)) if *n == 2.0)
            );
        }
        _ => panic!("expected `d` bound to a Dict"),
    }
}

#[test]
fn nested_dict_in_list_binds_correctly() {
    let region = run_root_storage();
    let captured: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    let test_run = run(r#"LET xs = [{"a": 1} {"b": 2}]"#, &region, captured);
    let scope = test_run.scope;
    let data = scope.bindings().data();
    match data.get("xs").map(|(o, _, _)| *o) {
        Some(KObject::List(outer, _)) => {
            assert_eq!(outer.len(), 2);
            match &outer[0] {
                Held::Object(KObject::Dict(d, _)) => assert!(matches!(
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
    // The list reaches `KKey::try_from_kobject` at materialization time and is rejected.
    let result =
        interpret_with_writer("LET k = [1 2]\nLET d = {(k): 1}", Box::new(std::io::sink()));
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
