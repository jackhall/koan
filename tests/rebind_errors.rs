//! Integration tests for the same-scope rebind rule and per-signature DuplicateOverload
//! check. Both error variants land in `KErrorKind::Rebind` / `DuplicateOverload`; the
//! tests assert via `read_result` since builtins propagate structured errors rather than
//! aborting `execute`.

use std::cell::RefCell;
use std::rc::Rc;

use koan::builtins::default_scope;
use koan::machine::model::KObject;
use koan::machine::{run_root_storage, FrameStorage, KError, KErrorKind, KoanRuntime, Scope};
use koan::parse::parse;

struct SharedBuf(Rc<RefCell<Vec<u8>>>);
impl std::io::Write for SharedBuf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn build_scope<'a>(region: &'a Rc<FrameStorage>) -> &'a Scope<'a> {
    let captured = Rc::new(RefCell::new(Vec::new()));
    default_scope(region, Box::new(SharedBuf(captured)))
}

fn run_collecting_errors<'a>(scope: &'a Scope<'a>, source: &str) -> Vec<Result<(), KError>> {
    let exprs = parse(source).expect("parse should succeed");
    let mut runtime = KoanRuntime::new();
    let mut ids = Vec::new();
    for e in exprs {
        ids.push(runtime.dispatch_in_scope(e, scope));
    }
    let _ = runtime.execute();
    // These tests assert only on `Ok`/`Err`, never on the produced value, so discard the carrier —
    // the scheduler re-anchors a read to its own borrow and the value need not escape it.
    ids.into_iter()
        .map(|id| runtime.result_error(id).map_err(|e| e.clone()))
        .collect()
}

/// `LET x = 1; LET x = 2` errors with `Rebind` on the second statement (same-scope
/// duplicate is rejected per the decided rule).
#[test]
fn same_scope_let_rebind_errors() {
    let region = run_root_storage();
    let scope = build_scope(&region);
    let results = run_collecting_errors(scope, "LET x = 1\nLET x = 2");
    assert!(results[0].is_ok(), "first LET should succeed");
    let err = match &results[1] {
        Err(e) => e,
        Ok(_) => panic!("second LET should error"),
    };
    assert!(
        matches!(&err.kind, KErrorKind::Rebind { name } if name == "x"),
        "expected Rebind, got {err}",
    );
}

/// `LET x = 1; LET x = (FN ...)` errors with `Rebind`. The function-bucket dedupe runs
/// in `bind_value` only when the slot is empty; once `data["x"]` holds a non-function,
/// any subsequent `LET x = ...` (function or otherwise) collides.
#[test]
fn let_function_collides_with_let_value() {
    let region = run_root_storage();
    let scope = build_scope(&region);
    let results = run_collecting_errors(
        scope,
        "LET x = 1\n\
         LET x = (FN (DOUBLE y :Number) -> Number = (y))",
    );
    assert!(results[0].is_ok());
    let err = match &results[1] {
        Err(e) => e,
        Ok(_) => panic!("rebinding x should error"),
    };
    assert!(
        matches!(&err.kind, KErrorKind::Rebind { name } if name == "x"),
        "expected Rebind, got {err}",
    );
}

/// Two FNs with the *exact same signature* (same shape + same per-slot KType) collide
/// with `DuplicateOverload`. The signature key is the per-untyped-shape bucket; the
/// exact-equal check inside `register_function` distinguishes a duplicate registration
/// from a same-shape overload with different KTypes.
#[test]
fn exact_signature_duplicate_errors() {
    let region = run_root_storage();
    let scope = build_scope(&region);
    let results = run_collecting_errors(
        scope,
        "FN (DOUBLE x :Number) -> Number = (x)\n\
         FN (DOUBLE x :Number) -> Number = (x)",
    );
    assert!(results[0].is_ok());
    let err = match &results[1] {
        Err(e) => e,
        Ok(_) => panic!("duplicate FN should error"),
    };
    assert!(
        matches!(&err.kind, KErrorKind::DuplicateOverload { name, .. } if name == "DOUBLE"),
        "expected DuplicateOverload, got {err}",
    );
}

/// Cross-scope shadowing still works — same name in a child scope (here, a MODULE body)
/// doesn't collide with the outer LET.
#[test]
fn cross_scope_shadowing_succeeds() {
    let region = run_root_storage();
    let scope = build_scope(&region);
    let results = run_collecting_errors(
        scope,
        "LET x = 1\n\
         MODULE Mod = (LET x = 99)",
    );
    assert!(results[0].is_ok(), "outer LET should succeed");
    assert!(
        results[1].is_ok(),
        "shadowing LET inside MODULE should succeed"
    );
    // Outer x stays 1.
    assert!(matches!(scope.lookup("x"), Some(KObject::Number(n)) if *n == 1.0));
    // Module's x is 99. A module is a value — its `&Module` rides the Object-arm value in `data`.
    let m = match scope.lookup("Mod") {
        Some(KObject::Module(m)) => *m,
        _ => panic!("Mod should bind a module value"),
    };
    let x = m.child_scope().lookup("x");
    assert!(matches!(x, Some(KObject::Number(n)) if *n == 99.0));
}

/// A user FN whose lead keyword + signature shape collides with a builtin's dispatch
/// bucket is rejected with `Rebind` — builtins are immutable and unshadowable, so a user
/// overload never merges into a builtin bucket. Routed through `interpret_with_writer` so
/// the top-level statement carries a real lexical chain (a user index, not the chain-less
/// `BUILTIN` fallback that bypasses the gate).
#[test]
fn user_fn_over_builtin_keyword_rejected() {
    let sink = Rc::new(RefCell::new(Vec::new()));
    let err = koan::machine::interpret_with_writer(
        "FN (PRINT x :Number) -> Null = (x)",
        Box::new(SharedBuf(sink)),
    )
    .expect_err("a user FN over the builtin PRINT bucket should error");
    assert!(
        matches!(&err.kind, KErrorKind::Rebind { name } if name == "PRINT"),
        "expected Rebind on PRINT, got {err}",
    );
}
