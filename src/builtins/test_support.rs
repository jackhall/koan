//! Shared test-only scaffolding for the builtin tests: a `Write` sink that captures PRINT
//! output, the parse/run/run_err harness over the dispatcher, run-root scope constructors,
//! and a handful of dispatch-test signature/marker builders shared between
//! `core::scope::tests` and `execute::scheduler::tests`.
//!
//! All items are `pub(crate)` and the module is gated `#[cfg(test)]` at its declaration site.

use std::cell::RefCell;
use std::io::Write;
use std::rc::Rc;

use crate::machine::model::KObject;
use crate::machine::core::kfunction::KFunction;
use crate::machine::model::types::{Argument, ExpressionSignature, KType, SignatureElement, ReturnType};
use crate::machine::{KError, RuntimeArena, Scope};
use crate::machine::execute::Scheduler;
use crate::machine::model::ast::KExpression;
use crate::parse::parse;

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
pub(crate) fn parse_one<'a>(src: &str) -> KExpression<'a> {
    let mut exprs = parse(src).expect("parse should succeed");
    assert_eq!(exprs.len(), 1, "test helper expects a single expression");
    exprs.remove(0)
}

/// Semantic errors surface via `read_result`, not `execute`; reach for [`run_one_err`] when
/// the test expects a `KError`.
///
/// Uses `add_dispatch` (not `enter_block`) so the submission picks up the detached
/// auto-root chain — visibility is "complete" against `scope`, so every binding from
/// prior `run(...)` calls reads through. This matches REPL semantics: a single
/// expression queried against an existing scope sees everything in it.
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

/// REPL-style setup: parse `source` and dispatch each top-level statement
/// individually via `add_dispatch`, so each picks up the detached auto-root chain
/// and reads through to every previously-bound name. Chained calls
/// (`run(scope, "...")` then `run(scope, "...")`) compose because each submission's
/// visibility is "complete" against `scope`. Tests that need to assert top-level
/// statement *ordering* (e.g. forward-ref-fails behavior) call `enter_block`
/// directly instead.
pub(crate) fn run<'a>(scope: &'a Scope<'a>, source: &str) {
    let exprs = parse(source).expect("parse should succeed");
    let mut sched = Scheduler::new();
    for expr in exprs {
        sched.add_dispatch(expr, scope);
    }
    sched.execute().expect("scheduler should succeed");
}

// =====================================================================
// Legacy-pinned helpers
//
// Step 3 of the stateful-dispatch refactor (commit 3d, ConstructorCall)
// drops NEWTYPE / TypeConstructor head support from the stateful driver
// (`stateful_constructor_call` surfaces `TypeMismatch` for those heads).
// Tests that exercise NEWTYPE constructor calls (`(Distance (3.0))`,
// `(Boxed (p))`, etc.) pin to the legacy driver via these helpers so the
// toggle-on suite stays green; step 4+ revisits whether those heads need
// stateful coverage.
// =====================================================================

/// Build a Scheduler pinned to the legacy `run_dispatch` driver regardless of
/// the `KOAN_STATEFUL_DISPATCH` env var. Used by [`run_legacy`],
/// [`run_one_legacy`], and [`run_one_err_legacy`].
pub(crate) fn sched_legacy<'a>() -> Scheduler<'a> {
    Scheduler::new().with_stateful_dispatch(false)
}

/// Legacy-pinned counterpart to [`run`]. Each top-level statement runs on
/// `run_dispatch` so NEWTYPE / TypeConstructor head support is preserved.
pub(crate) fn run_legacy<'a>(scope: &'a Scope<'a>, source: &str) {
    let exprs = parse(source).expect("parse should succeed");
    let mut sched = sched_legacy();
    for expr in exprs {
        sched.add_dispatch(expr, scope);
    }
    sched.execute().expect("scheduler should succeed");
}

/// Legacy-pinned counterpart to [`run_one`].
pub(crate) fn run_one_legacy<'a>(scope: &'a Scope<'a>, expr: KExpression<'a>) -> &'a KObject<'a> {
    let mut sched = sched_legacy();
    let id = sched.add_dispatch(expr, scope);
    sched.execute().expect("scheduler should succeed");
    sched.read(id)
}

/// Legacy-pinned counterpart to [`run_one_err`].
pub(crate) fn run_one_err_legacy<'a>(scope: &'a Scope<'a>, expr: KExpression<'a>) -> KError {
    let mut sched = sched_legacy();
    let id = sched.add_dispatch(expr, scope);
    sched.execute().expect("scheduler should not surface errors directly");
    match sched.read_result(id) {
        Ok(_) => panic!("expected error"),
        Err(e) => e.clone(),
    }
}

/// Fetch the single bare-`FN` overload whose signature's first keyword is `keyword`,
/// searching the `functions` dispatch buckets. Bare FN keywords no longer mirror into
/// `data` (only `LET f = (FN …)` does), so tests that inspect a registered function's
/// signature read it from the dispatch surface through this helper. Panics if no overload
/// or more than one is found under `keyword`.
pub(crate) fn lookup_fn<'a>(scope: &'a Scope<'a>, keyword: &str) -> &'a KFunction<'a> {
    let mut found: Option<&'a KFunction<'a>> = None;
    for (_, bucket) in scope.bindings().iter_functions() {
        for f in bucket {
            let first_kw = f.signature.elements.iter().find_map(|e| match e {
                SignatureElement::Keyword(s) => Some(s.as_str()),
                _ => None,
            });
            if first_kw == Some(keyword) {
                assert!(found.is_none(), "ambiguous: multiple overloads under `{keyword}`");
                found = Some(f);
            }
        }
    }
    found.unwrap_or_else(|| panic!("no FN overload registered under `{keyword}`"))
}

/// True iff some `functions` bucket holds an overload whose first keyword is `keyword`.
/// Negative-path companion to [`lookup_fn`] for "this FN should not register" assertions
/// (which can no longer be expressed as `data.get(keyword).is_none()` now that bare FN
/// keywords never land in `data`).
pub(crate) fn fn_is_registered(scope: &Scope<'_>, keyword: &str) -> bool {
    scope.bindings().iter_functions().into_iter().any(|(_, bucket)| {
        bucket.iter().any(|f| {
            f.signature.elements.iter().find_map(|e| match e {
                SignatureElement::Keyword(s) => Some(s.as_str()),
                _ => None,
            }) == Some(keyword)
        })
    })
}

/// Allocate a labeled marker object on `scope`'s arena. Dispatch tests register builtins
/// whose bodies return distinct markers (`"identifier"`, `"any"`, …) so the test asserts
/// which overload won by inspecting the produced string.
pub(crate) fn marker<'a>(scope: &'a Scope<'a>, label: &'static str) -> &'a KObject<'a> {
    scope.arena.alloc(KObject::KString(label.into()))
}

/// Build a one-argument signature (`<name: kt>`) returning `Any`. Used by dispatch-test
/// builtins on both sides of the scope/scheduler split.
pub(crate) fn one_slot_sig<'a>(name: &str, kt: KType<'a>) -> ExpressionSignature<'a> {
    ExpressionSignature {
        return_type: ReturnType::Resolved(KType::Any),
        elements: vec![SignatureElement::Argument(Argument {
            name: name.into(),
            ktype: kt,
        })],
    }
}
