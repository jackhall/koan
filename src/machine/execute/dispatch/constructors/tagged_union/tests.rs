use std::cell::RefCell;
use std::io::Write;
use std::rc::Rc;

use crate::builtins::default_scope;
use crate::machine::core::{run_root_storage, FrameStorage, KErrorKind, Scope};
use crate::machine::execute::KoanRuntime;
use crate::machine::model::ast::KExpression;
use crate::machine::model::values::KObject;
use crate::parse::parse;

struct SharedBuf(Rc<RefCell<Vec<u8>>>);
impl Write for SharedBuf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn build_scope<'run>(
    region: &'run Rc<FrameStorage>,
    captured: Rc<RefCell<Vec<u8>>>,
) -> &'run Scope<'run> {
    default_scope(region, Box::new(SharedBuf(captured)))
}

fn parse_one<'run>(src: &str) -> KExpression<'run> {
    let mut exprs = parse(src).expect("parse should succeed");
    assert_eq!(exprs.len(), 1, "test helper expects a single expression");
    exprs.remove(0)
}

fn run<'run>(scope: &'run Scope<'run>, source: &str) {
    let exprs = parse(source).expect("parse should succeed");
    let mut runtime = KoanRuntime::new();
    for expr in exprs {
        runtime.dispatch_in_scope(expr, scope);
    }
    runtime.execute().expect("scheduler should succeed");
}

fn run_one<'run>(scope: &'run Scope<'run>, expr: KExpression<'run>) -> &'run KObject<'run> {
    let mut runtime = KoanRuntime::new();
    let id = runtime.dispatch_in_scope(expr, scope);
    runtime.execute().expect("scheduler should succeed");
    // The frameless top-level terminal outlives the local `runtime`; widen the scheduler's `'node`
    // read to the scope lifetime (see `test_support::extract_terminal`).
    crate::builtins::test_support::extract_terminal(&runtime, scope, id).object()
}

fn run_one_err<'run>(
    scope: &'run Scope<'run>,
    expr: KExpression<'run>,
) -> crate::machine::core::KError {
    let mut runtime = KoanRuntime::new();
    let id = runtime.dispatch_in_scope(expr, scope);
    runtime
        .execute()
        .expect("scheduler should not surface errors directly");
    match runtime.result_error(id) {
        Ok(()) => panic!("expected error"),
        Err(e) => e.clone(),
    }
}

/// The tagged-union value-type check fires when the value-cell resolves to a
/// `KObject` that doesn't match the tag's expected type.
#[test]
fn ctor_fast_lane_rejects_value_of_wrong_type() {
    let region = run_root_storage();
    let captured = Rc::new(RefCell::new(Vec::new()));
    let scope = build_scope(&region, captured);
    run(scope, "UNION Maybe = (Some :Number None :Null)");
    let err = run_one_err(scope, parse_one("Maybe (Some \"oops\")"));
    match &err.kind {
        KErrorKind::TypeMismatch { arg, expected, got } => {
            assert_eq!(arg, "value");
            assert_eq!(expected, "Number");
            assert_eq!(got, "Str");
        }
        _ => panic!("expected TypeMismatch on value, got {err}"),
    }
}

/// `TypeCall` fast lane (leaf-Type head) propagates the schema's tag check.
#[test]
fn ctor_fast_lane_propagates_tag_validation_error() {
    let region = run_root_storage();
    let captured = Rc::new(RefCell::new(Vec::new()));
    let scope = build_scope(&region, captured);
    run(scope, "UNION Maybe = (Some :Number None :Null)");
    let err = run_one_err(scope, parse_one("Maybe (Other 42)"));
    assert!(
        matches!(&err.kind, KErrorKind::ShapeError(msg) if msg.contains("`Other`")),
        "expected ShapeError mentioning `Other`, got {err}",
    );
}

/// Value-cell sub-expression `(x)` rides the `BareIdentifier` fast lane to resolve
/// `x` before the newtype construction sees the value bind. A user-union variant value is an
/// ordinary `KObject::Wrapped` over the member `SetRef`, not a `KObject::Tagged`.
#[test]
fn ctor_fast_lane_with_sub_expression_value() {
    use crate::machine::model::types::KType;
    let region = run_root_storage();
    let captured = Rc::new(RefCell::new(Vec::new()));
    let scope = build_scope(&region, captured);
    run(scope, "UNION Maybe = (Some :Number None :Null)\nLET x = 7");
    let result = run_one(scope, parse_one("Maybe (Some (x))"));
    match result {
        KObject::Wrapped { inner, type_id } => {
            assert!(matches!(inner.get(), KObject::Number(n) if *n == 7.0));
            match type_id {
                KType::SetRef { set, index } => assert_eq!(set.member(*index).name, "Some"),
                other => panic!("expected a member SetRef type_id, got {other:?}"),
            }
        }
        other => panic!("expected Wrapped, got {:?}", other.ktype()),
    }
}
