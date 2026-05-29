use std::cell::RefCell;
use std::io::Write;
use std::rc::Rc;

use crate::machine::model::ast::KExpression;
use crate::parse::parse;
use crate::builtins::default_scope;
use crate::machine::core::{KErrorKind, RuntimeArena, Scope};
use crate::machine::execute::Scheduler;
use crate::machine::model::values::KObject;

struct SharedBuf(Rc<RefCell<Vec<u8>>>);
impl Write for SharedBuf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}

fn build_scope<'a>(arena: &'a RuntimeArena, captured: Rc<RefCell<Vec<u8>>>) -> &'a Scope<'a> {
    default_scope(arena, Box::new(SharedBuf(captured)))
}

fn parse_one<'a>(src: &str) -> KExpression<'a> {
    let mut exprs = parse(src).expect("parse should succeed");
    assert_eq!(exprs.len(), 1, "test helper expects a single expression");
    exprs.remove(0)
}

fn run<'a>(scope: &'a Scope<'a>, source: &str) {
    let exprs = parse(source).expect("parse should succeed");
    let mut sched = Scheduler::new();
    for expr in exprs {
        sched.add_dispatch(expr, scope);
    }
    sched.execute().expect("scheduler should succeed");
}

fn run_one<'a>(scope: &'a Scope<'a>, expr: KExpression<'a>) -> &'a KObject<'a> {
    let mut sched = Scheduler::new();
    let id = sched.add_dispatch(expr, scope);
    sched.execute().expect("scheduler should succeed");
    sched.read(id)
}

fn run_one_err<'a>(
    scope: &'a Scope<'a>,
    expr: KExpression<'a>,
) -> crate::machine::core::KError {
    let mut sched = Scheduler::new();
    let id = sched.add_dispatch(expr, scope);
    sched.execute().expect("scheduler should not surface errors directly");
    match sched.read_result(id) {
        Ok(_) => panic!("expected error"),
        Err(e) => e.clone(),
    }
}

#[test]
fn struct_construction_via_type_token() {
    let arena = RuntimeArena::new();
    let captured = Rc::new(RefCell::new(Vec::new()));
    let scope = build_scope(&arena, captured);
    run(scope, "STRUCT Point = (x :Number, y :Number)");
    let result = run_one(scope, parse_one("Point (x = 3, y = 4)"));
    match result {
        KObject::Struct { name: type_name, fields, .. } => {
            assert_eq!(type_name, "Point");
            assert_eq!(fields.len(), 2);
            assert!(matches!(fields.get("x"), Some(KObject::Number(n)) if *n == 3.0));
            assert!(matches!(fields.get("y"), Some(KObject::Number(n)) if *n == 4.0));
        }
        other => panic!("expected Struct, got {:?}", other.ktype()),
    }
}

#[test]
fn struct_construction_missing_field_errors() {
    let arena = RuntimeArena::new();
    let captured = Rc::new(RefCell::new(Vec::new()));
    let scope = build_scope(&arena, captured);
    run(scope, "STRUCT Point = (x :Number, y :Number)");
    let err = run_one_err(scope, parse_one("Point (x = 3)"));
    assert!(
        matches!(&err.kind, KErrorKind::MissingArg(name) if name == "y"),
        "expected MissingArg(\"y\"), got {err}",
    );
}

#[test]
fn struct_construction_unknown_field_errors() {
    let arena = RuntimeArena::new();
    let captured = Rc::new(RefCell::new(Vec::new()));
    let scope = build_scope(&arena, captured);
    run(scope, "STRUCT Point = (x :Number, y :Number)");
    let err = run_one_err(scope, parse_one("Point (x = 3, y = 4, z = 5)"));
    assert!(
        matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("unknown field") && msg.contains("`z`")),
        "expected ShapeError on unknown field z, got {err}",
    );
}

#[test]
fn struct_construction_value_type_mismatch() {
    let arena = RuntimeArena::new();
    let captured = Rc::new(RefCell::new(Vec::new()));
    let scope = build_scope(&arena, captured);
    run(scope, "STRUCT Point = (x :Number, y :Number)");
    let err = run_one_err(scope, parse_one("Point (x = 3, y = \"oops\")"));
    match &err.kind {
        KErrorKind::TypeMismatch { arg, expected, got } => {
            assert_eq!(arg, "y");
            assert_eq!(expected, "Number");
            assert_eq!(got, "Str");
        }
        _ => panic!("expected TypeMismatch on field y, got {err}"),
    }
}

#[test]
fn struct_construction_with_identifier_arg() {
    // Bare identifiers on the value side resolve through the `BareIdentifier` fast lane
    // because `apply` wraps each value-part in a single-part sub-expression after reordering.
    let arena = RuntimeArena::new();
    let captured = Rc::new(RefCell::new(Vec::new()));
    let scope = build_scope(&arena, captured);
    run(scope, "STRUCT Point = (x :Number, y :Number)\nLET ax = 7\nLET ay = 9");
    let result = run_one(scope, parse_one("Point (x = ax, y = ay)"));
    match result {
        KObject::Struct { fields, .. } => {
            assert!(matches!(fields.get("x"), Some(KObject::Number(n)) if *n == 7.0));
            assert!(matches!(fields.get("y"), Some(KObject::Number(n)) if *n == 9.0));
        }
        other => panic!("expected Struct, got {:?}", other.ktype()),
    }
}

#[test]
fn struct_construction_order_independent() {
    // The user can write fields in any order; `apply` reorders to schema declaration order
    // before construction. Result is identical regardless of source order.
    let arena = RuntimeArena::new();
    let captured = Rc::new(RefCell::new(Vec::new()));
    let scope = build_scope(&arena, captured);
    run(scope, "STRUCT Point = (x :Number, y :Number)");
    let result = run_one(scope, parse_one("Point (y = 4, x = 3)"));
    match result {
        KObject::Struct { fields, .. } => {
            assert!(matches!(fields.get("x"), Some(KObject::Number(n)) if *n == 3.0));
            assert!(matches!(fields.get("y"), Some(KObject::Number(n)) if *n == 4.0));
        }
        other => panic!("expected Struct, got {:?}", other.ktype()),
    }
}

#[test]
fn struct_construction_missing_colon_errors() {
    let arena = RuntimeArena::new();
    let captured = Rc::new(RefCell::new(Vec::new()));
    let scope = build_scope(&arena, captured);
    run(scope, "STRUCT Point = (x :Number, y :Number)");
    let err = run_one_err(scope, parse_one("Point (x 3, y 4)"));
    assert!(
        matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("`:`") || msg.contains("separator") || msg.contains("triples")),
        "expected ShapeError on missing colon, got {err}",
    );
}

#[test]
fn struct_construction_duplicate_name_errors() {
    let arena = RuntimeArena::new();
    let captured = Rc::new(RefCell::new(Vec::new()));
    let scope = build_scope(&arena, captured);
    run(scope, "STRUCT Point = (x :Number, y :Number)");
    let err = run_one_err(scope, parse_one("Point (x = 1, x = 2)"));
    assert!(
        matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("duplicate") && msg.contains("`x`")),
        "expected ShapeError on duplicate name, got {err}",
    );
}

#[test]
fn struct_construction_unbound_type_token_errors() {
    let arena = RuntimeArena::new();
    let captured = Rc::new(RefCell::new(Vec::new()));
    let scope = build_scope(&arena, captured);
    let err = run_one_err(scope, parse_one("Bogus (x = 1, y = 2)"));
    assert!(
        matches!(&err.kind, KErrorKind::UnboundName(name) if name == "Bogus"),
        "expected UnboundName(\"Bogus\"), got {err}",
    );
}

/// Regression: struct values iterate (and therefore PRINT/`summarize` render) in
/// declaration order. Pre-Phase-1 this used a `HashMap`, so the surface output sat at
/// hash-iteration order — which differed from the schema and surprised users. The order
/// `z, a, m` is chosen to differ from any alphabetical / hash-stable ordering on a small
/// set of single-letter keys.
#[test]
fn struct_value_iterates_in_declaration_order() {
    let arena = RuntimeArena::new();
    let captured = Rc::new(RefCell::new(Vec::new()));
    let scope = build_scope(&arena, captured);
    run(scope, "STRUCT Triple = (z :Number, a :Number, m :Number)");
    let result = run_one(scope, parse_one("Triple (a = 2, m = 3, z = 1)"));
    match result {
        KObject::Struct { fields, .. } => {
            let keys: Vec<&str> = fields.keys().map(|s| s.as_str()).collect();
            assert_eq!(
                keys,
                vec!["z", "a", "m"],
                "struct fields should iterate in declaration order, not call-site order",
            );
        }
        other => panic!("expected Struct, got {:?}", other.ktype()),
    }
    let summary = crate::machine::model::types::Parseable::summarize(result);
    assert_eq!(
        summary, "Triple(z: 1, a: 2, m: 3)",
        "summary must emit fields in declaration order"
    );
}

#[test]
fn struct_value_summarizes_with_type_name_and_fields() {
    // Smoke-tests the `summarize` format so PRINT downstream doesn't surprise users.
    let arena = RuntimeArena::new();
    let captured = Rc::new(RefCell::new(Vec::new()));
    let scope = build_scope(&arena, captured);
    run(scope, "STRUCT Point = (x :Number, y :Number)");
    let result = run_one(scope, parse_one("Point (x = 3, y = 4)"));
    let summary = crate::machine::model::types::Parseable::summarize(result);
    assert!(summary.starts_with("Point("), "summary should start with Point(, got {summary}");
    assert!(summary.contains("x: 3"), "summary should include x: 3, got {summary}");
    assert!(summary.contains("y: 4"), "summary should include y: 4, got {summary}");
}
