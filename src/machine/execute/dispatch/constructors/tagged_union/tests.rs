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

/// `tagged_union::construct`'s value-type check fires when the value-cell
/// resolves to a `KObject` that doesn't match the tag's expected type.
#[test]
fn ctor_fast_lane_rejects_value_of_wrong_type() {
    let arena = RuntimeArena::new();
    let captured = Rc::new(RefCell::new(Vec::new()));
    let scope = build_scope(&arena, captured);
    run(scope, "UNION Maybe = (some :Number none :Null)");
    let err = run_one_err(scope, parse_one("Maybe (some \"oops\")"));
    match &err.kind {
        KErrorKind::TypeMismatch { arg, expected, got } => {
            assert_eq!(arg, "value");
            assert_eq!(expected, "Number");
            assert_eq!(got, "Str");
        }
        _ => panic!("expected TypeMismatch on value, got {err}"),
    }
}

/// `ConstructorCall` fast lane (leaf-Type head) propagates the schema's tag check.
#[test]
fn ctor_fast_lane_propagates_tag_validation_error() {
    let arena = RuntimeArena::new();
    let captured = Rc::new(RefCell::new(Vec::new()));
    let scope = build_scope(&arena, captured);
    run(scope, "UNION Maybe = (some :Number none :Null)");
    let err = run_one_err(scope, parse_one("Maybe (other 42)"));
    assert!(
        matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("`other`")),
        "expected ShapeError mentioning `other`, got {err}",
    );
}

/// Value-cell sub-expression `(x)` rides the `BareIdentifier` fast lane to resolve
/// `x` before the synthesized TAG call sees the typed-slot bind.
#[test]
fn ctor_fast_lane_with_sub_expression_value() {
    let arena = RuntimeArena::new();
    let captured = Rc::new(RefCell::new(Vec::new()));
    let scope = build_scope(&arena, captured);
    run(scope, "UNION Maybe = (some :Number none :Null)\nLET x = 7");
    let result = run_one(scope, parse_one("Maybe (some (x))"));
    match result {
        KObject::Tagged { tag, value, .. } => {
            assert_eq!(tag, "some");
            assert!(matches!(&**value, KObject::Number(n) if *n == 7.0));
        }
        other => panic!("expected Tagged, got {:?}", other.ktype()),
    }
}
