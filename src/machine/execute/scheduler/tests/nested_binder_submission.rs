//! Regression test for recursive nested-binder submission at the outermost
//! submission point — see `roadmap/dispatch_fix/nested-binder-submission.md`.
//!
//! Submit `LET f = (FN (HELPER x :Number) -> Number = (x))` via
//! `add_dispatch`. The outer LET is a binder (installs placeholder `f`); the
//! inner FN is also a binder (installs placeholder `HELPER`). Before the
//! recursive-submission fix, the inner FN's placeholder installed only when
//! LET's Phase 4 spawned the sub-Dispatch — after a sibling could pop under
//! FIFO. Under strict-only admission, any sibling that dispatches first would
//! hard-error on `HELPER` instead of parking.
//!
//! The expected post-fix invariant: after the outer submission returns and
//! before any node runs, BOTH `f` AND `HELPER` are present in the dispatching
//! scope's `placeholders` map.

use std::io::Write;

use crate::builtins::default_scope;
use crate::machine::execute::Scheduler;
use crate::machine::RuntimeArena;
use crate::parse::parse;

struct Sink;
impl Write for Sink {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> { Ok(b.len()) }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}

#[test]
fn nested_binder_installs_inner_placeholder_at_outer_submission() {
    let arena = RuntimeArena::new();
    let scope = default_scope(&arena, Box::new(Sink));
    let mut exprs =
        parse("LET f = (FN (HELPER x :Number) -> Number = (x))").expect("parse should succeed");
    assert_eq!(exprs.len(), 1, "test fixture: single top-level expression");
    let expr = exprs.remove(0);
    let mut sched = Scheduler::new();
    let _id = sched.add_dispatch(expr, scope);
    // CRITICAL: read placeholders BEFORE `execute()` — the fix is that the
    // installs happen at *submission* time, not run time.
    let placeholders = scope.bindings().placeholders();
    assert!(
        placeholders.contains_key("f"),
        "outer LET should install placeholder `f` at submission; \
         placeholders = {:?}",
        placeholders.keys().collect::<Vec<_>>(),
    );
    assert!(
        placeholders.contains_key("HELPER"),
        "inner FN (pre-submitted as a sub-Dispatch of LET) should install \
         placeholder `HELPER` at submission; placeholders = {:?}",
        placeholders.keys().collect::<Vec<_>>(),
    );
}
