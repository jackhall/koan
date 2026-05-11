//! Shared test-only scaffolding for the builtin tests: a `Write` sink that captures PRINT
//! output, the parse/run/run_err harness over the dispatcher, and run-root scope constructors.
//!
//! All items are `pub(crate)` and the module is gated `#[cfg(test)]` at its declaration site.

use std::cell::RefCell;
use std::io::Write;
use std::rc::Rc;

use crate::dispatch::{KError, KObject, RuntimeArena, Scope};
use crate::execute::scheduler::Scheduler;
use crate::parse::expression_tree::parse;
use crate::parse::kexpression::KExpression;

use super::default_scope;

/// `Write` adapter that mirrors output into a shared `Vec<u8>` so tests can read back what
/// the scope's sink received.
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

pub(crate) fn run_root_with_buf<'a>(
    arena: &'a RuntimeArena,
) -> (&'a Scope<'a>, Rc<RefCell<Vec<u8>>>) {
    let buf = Rc::new(RefCell::new(Vec::new()));
    let scope = default_scope(arena, Box::new(SharedBuf(buf.clone())));
    (scope, buf)
}

pub(crate) fn run_root_silent<'a>(arena: &'a RuntimeArena) -> &'a Scope<'a> {
    default_scope(arena, Box::new(std::io::sink()))
}

/// Run-root scope with no builtins registered, for tests that exercise scope-machinery
/// primitives directly without going through `default_scope`.
pub(crate) fn run_root_bare<'a>(arena: &'a RuntimeArena) -> &'a Scope<'a> {
    arena.alloc_scope(Scope::run_root(arena, None, Box::new(std::io::sink())))
}

/// Parse a source string expected to contain exactly one top-level expression. Panics on
/// parse failure or wrong arity.
pub(crate) fn parse_one(src: &str) -> KExpression<'static> {
    let mut exprs = parse(src).expect("parse should succeed");
    assert_eq!(exprs.len(), 1, "test helper expects a single expression");
    exprs.remove(0)
}

/// Semantic errors surface via `read_result`, not `execute`; reach for [`run_one_err`] when
/// the test expects a `KError`.
pub(crate) fn run_one<'a>(scope: &'a Scope<'a>, expr: KExpression<'a>) -> &'a KObject<'a> {
    let mut sched = Scheduler::new();
    let id = sched.add_dispatch(expr, scope);
    sched.execute().expect("scheduler should succeed");
    sched.read(id)
}

/// Like [`run_one`] but returns the `KError` produced by the dispatched node. Panics if the
/// node finished without an error.
pub(crate) fn run_one_err<'a>(scope: &'a Scope<'a>, expr: KExpression<'a>) -> KError {
    let mut sched = Scheduler::new();
    let id = sched.add_dispatch(expr, scope);
    sched.execute().expect("scheduler should not surface errors directly");
    match sched.read_result(id) {
        Ok(_) => panic!("expected error"),
        Err(e) => e.clone(),
    }
}

pub(crate) fn run<'a>(scope: &'a Scope<'a>, source: &str) {
    let exprs = parse(source).expect("parse should succeed");
    let mut sched = Scheduler::new();
    for expr in exprs {
        sched.add_dispatch(expr, scope);
    }
    sched.execute().expect("scheduler should succeed");
}
