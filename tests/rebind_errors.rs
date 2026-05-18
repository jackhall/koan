//! Integration tests for the same-scope rebind rule and per-signature DuplicateOverload
//! check. Both error variants land in `KErrorKind::Rebind` / `DuplicateOverload`; the
//! tests assert via `read_result` since builtins propagate structured errors rather than
//! aborting `execute`.

use std::cell::RefCell;
use std::rc::Rc;

use koan::builtins::default_scope;
use koan::machine::model::KObject;
use koan::machine::{KError, KErrorKind, RuntimeArena, Scheduler, Scope};
use koan::parse::parse;

struct SharedBuf(Rc<RefCell<Vec<u8>>>);
impl std::io::Write for SharedBuf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}

fn build_scope<'a>(arena: &'a RuntimeArena) -> &'a Scope<'a> {
    let captured = Rc::new(RefCell::new(Vec::new()));
    default_scope(arena, Box::new(SharedBuf(captured)))
}

fn run_collecting_errors<'a>(
    scope: &'a Scope<'a>,
    source: &str,
) -> Vec<Result<&'a KObject<'a>, KError>> {
    let exprs = parse(source).expect("parse should succeed");
    let mut sched = Scheduler::new();
    let mut ids = Vec::new();
    for e in exprs { ids.push(sched.add_dispatch(e, scope)); }
    let _ = sched.execute();
    ids.into_iter()
        .map(|id| sched.read_result(id).map_err(|e| e.clone()))
        .collect()
}

/// `LET x = 1; LET x = 2` errors with `Rebind` on the second statement (same-scope
/// duplicate is rejected per the decided rule).
#[test]
fn same_scope_let_rebind_errors() {
    let arena = RuntimeArena::new();
    let scope = build_scope(&arena);
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
    let arena = RuntimeArena::new();
    let scope = build_scope(&arena);
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
    let arena = RuntimeArena::new();
    let scope = build_scope(&arena);
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
    let arena = RuntimeArena::new();
    let scope = build_scope(&arena);
    let results = run_collecting_errors(
        scope,
        "LET x = 1\n\
         MODULE Mod = (LET x = 99)",
    );
    assert!(results[0].is_ok(), "outer LET should succeed");
    assert!(results[1].is_ok(), "shadowing LET inside MODULE should succeed");
    // Outer x stays 1.
    assert!(matches!(scope.lookup("x"), Some(KObject::Number(n)) if *n == 1.0));
    // Module's x is 99.
    let m = match scope.lookup("Mod") {
        Some(KObject::KModule(m, _)) => *m,
        _ => panic!("Mod should be a module"),
    };
    let data = m.child_scope().bindings().data();
    assert!(matches!(data.get("x"), Some(KObject::Number(n)) if *n == 99.0));
}
