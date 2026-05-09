//! Shared test-only scaffolding for the builtin tests. Centralizes the boilerplate that 13 of
//! the 18 builtin files used to copy verbatim — `SharedBuf` for capturing PRINT output, the
//! parse/run/run_err harness over the dispatcher, and the silent run-root constructor used by
//! tests that don't care what builtins write.
//!
//! All items are `pub(crate)` and gated under `#[cfg(test)]` at the module declaration site
//! ([`super::test_support`](super::test_support)), so this module is invisible outside the
//! test build.

use std::cell::RefCell;
use std::io::Write;
use std::rc::Rc;

use crate::dispatch::runtime::{KError, RuntimeArena, Scope};
use crate::dispatch::values::KObject;
use crate::execute::scheduler::Scheduler;
use crate::parse::expression_tree::parse;
use crate::parse::kexpression::KExpression;

use super::default_scope;

/// `Write` adapter that mirrors output into a shared `Vec<u8>`. Tests pass an `Rc<RefCell<...>>`
/// to `default_scope` (via `run_root_with_buf`) and then read the bytes back to assert what
/// PRINT (or any builtin that writes to the scope's sink) produced.
pub(crate) struct SharedBuf(pub Rc<RefCell<Vec<u8>>>);

impl Write for SharedBuf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Build a run-root scope wired up with the full builtin set, capturing all writes into a
/// fresh `Rc<RefCell<Vec<u8>>>` returned alongside the scope. Equivalent to the per-file
/// `build_scope` + `SharedBuf` pair that 13 builtin files used to redefine.
pub(crate) fn run_root_with_buf<'a>(
    arena: &'a RuntimeArena,
) -> (&'a Scope<'a>, Rc<RefCell<Vec<u8>>>) {
    let buf = Rc::new(RefCell::new(Vec::new()));
    let scope = default_scope(arena, Box::new(SharedBuf(buf.clone())));
    (scope, buf)
}

/// Build a run-root scope whose output sink discards everything. Used by tests that exercise
/// dispatch behavior but never assert on PRINT output. Wires up the full `default_scope`
/// builtin set, so dispatch behaves the same as a real run with `interpret`.
pub(crate) fn run_root_silent<'a>(arena: &'a RuntimeArena) -> &'a Scope<'a> {
    default_scope(arena, Box::new(std::io::sink()))
}

/// Build a bare run-root scope with no builtins registered. Used by the few tests that
/// exercise scope-machinery primitives (LET's `body` directly, dispatcher pre-run wiring)
/// where the full builtin set is irrelevant. Mirrors the
/// `arena.alloc_scope(Scope::run_root(&arena, None, Box::new(std::io::sink())))` pattern
/// without going through `default_scope`.
pub(crate) fn run_root_bare<'a>(arena: &'a RuntimeArena) -> &'a Scope<'a> {
    arena.alloc_scope(Scope::run_root(arena, None, Box::new(std::io::sink())))
}

/// Parse a source string that is expected to contain exactly one top-level expression. Panics
/// on parse failure or wrong arity — both indicate a malformed test fixture.
pub(crate) fn parse_one(src: &str) -> KExpression<'static> {
    let mut exprs = parse(src).expect("parse should succeed");
    assert_eq!(exprs.len(), 1, "test helper expects a single expression");
    exprs.remove(0)
}

/// Dispatch `expr` against `scope`, run the scheduler to completion, and return the resulting
/// `KObject`. Asserts the scheduler itself doesn't surface an error (semantic errors are
/// surfaced via `read_result`, not `execute`); reach for [`run_one_err`] when the test expects
/// a `KError`.
pub(crate) fn run_one<'a>(scope: &'a Scope<'a>, expr: KExpression<'a>) -> &'a KObject<'a> {
    let mut sched = Scheduler::new();
    let id = sched.add_dispatch(expr, scope);
    sched.execute().expect("scheduler should succeed");
    sched.read(id)
}

/// Like [`run_one`] but returns the `KError` produced by the dispatched node. Panics if the
/// node finished without an error — that means the test fixture's expectation is wrong.
pub(crate) fn run_one_err<'a>(scope: &'a Scope<'a>, expr: KExpression<'a>) -> KError {
    let mut sched = Scheduler::new();
    let id = sched.add_dispatch(expr, scope);
    sched.execute().expect("scheduler should not surface errors directly");
    match sched.read_result(id) {
        Ok(_) => panic!("expected error"),
        Err(e) => e.clone(),
    }
}

/// Multi-expression form of [`run_one`] that drops the per-node results — used by tests that
/// only care about the cumulative scope state (e.g., that `LET x = 1` followed by `LET y = 2`
/// inserted both names) rather than any specific dispatch's return value.
pub(crate) fn run<'a>(scope: &'a Scope<'a>, source: &str) {
    let exprs = parse(source).expect("parse should succeed");
    let mut sched = Scheduler::new();
    for expr in exprs {
        sched.add_dispatch(expr, scope);
    }
    sched.execute().expect("scheduler should succeed");
}
