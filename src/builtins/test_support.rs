//! Shared test-only scaffolding for the builtin tests: PRINT-capturing `Write` sink,
//! parse/run/run_err harness over the dispatcher, run-root scope constructors, and
//! dispatch-test signature/marker builders.

use std::cell::RefCell;
use std::io::Write;
use std::rc::Rc;

use crate::machine::core::kfunction::KFunction;
use crate::machine::execute::Scheduler;
use crate::machine::model::ast::KExpression;
use crate::machine::model::types::{
    Argument, ExpressionSignature, KType, ReturnType, SignatureElement,
};
use crate::machine::model::KObject;
use crate::machine::{KError, RuntimeArena, Scope};
use crate::parse::parse;

use super::default_scope;

/// `Write` adapter that mirrors output into a shared `Vec<u8>` so tests can read it back.
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

/// Run-root scope with no builtins registered, for tests that exercise scope machinery
/// directly.
pub(crate) fn run_root_bare<'a>(arena: &'a RuntimeArena) -> &'a Scope<'a> {
    arena.alloc_scope(Scope::run_root(arena, None, Box::new(std::io::sink())))
}

/// Parse a source string expected to contain exactly one top-level expression.
pub(crate) fn parse_one<'a>(src: &str) -> KExpression<'a> {
    let mut exprs = parse(src).expect("parse should succeed");
    assert_eq!(exprs.len(), 1, "test helper expects a single expression");
    exprs.remove(0)
}

/// Dispatches `expr` against `scope` with REPL-style "complete" visibility, so bindings
/// from prior `run(...)` calls read through. Semantic errors surface via `read_result`,
/// not `execute` — use [`run_one_err`] when the test expects a `KError`.
pub(crate) fn run_one<'a>(scope: &'a Scope<'a>, expr: KExpression<'a>) -> &'a KObject<'a> {
    let mut sched = Scheduler::new();
    let id = sched.add_dispatch(expr, scope);
    sched.execute().expect("scheduler should succeed");
    sched.read(id).object()
}

/// Like [`run_one`] but returns the `KError` produced by the dispatched node.
pub(crate) fn run_one_err<'a>(scope: &'a Scope<'a>, expr: KExpression<'a>) -> KError {
    let mut sched = Scheduler::new();
    let id = sched.add_dispatch(expr, scope);
    sched
        .execute()
        .expect("scheduler should not surface errors directly");
    match sched.read_result(id) {
        Ok(_) => panic!("expected error"),
        Err(e) => e.clone(),
    }
}

/// REPL-style setup: parse `source` and dispatch each top-level statement individually,
/// so chained `run(scope, ...)` calls compose. Tests asserting top-level statement
/// *ordering* (e.g. forward-ref-fails behavior) call `enter_block` directly instead.
pub(crate) fn run<'a>(scope: &'a Scope<'a>, source: &str) {
    let exprs = parse(source).expect("parse should succeed");
    let mut sched = Scheduler::new();
    for expr in exprs {
        sched.add_dispatch(expr, scope);
    }
    sched.execute().expect("scheduler should succeed");
}

/// Fetch the single bare-`FN` overload whose signature's first keyword is `keyword`.
/// Panics if zero or more than one match.
pub(crate) fn lookup_fn<'a>(scope: &'a Scope<'a>, keyword: &str) -> &'a KFunction<'a> {
    let mut found: Option<&'a KFunction<'a>> = None;
    for (_, bucket) in scope.bindings().iter_functions() {
        for f in bucket {
            let first_kw = f.signature.elements.iter().find_map(|e| match e {
                SignatureElement::Keyword(s) => Some(s.as_str()),
                _ => None,
            });
            if first_kw == Some(keyword) {
                assert!(
                    found.is_none(),
                    "ambiguous: multiple overloads under `{keyword}`"
                );
                found = Some(f);
            }
        }
    }
    found.unwrap_or_else(|| panic!("no FN overload registered under `{keyword}`"))
}

/// True iff some `functions` bucket holds an overload whose first keyword is `keyword`.
/// Negative-path companion to [`lookup_fn`] for "this FN should not register" assertions.
pub(crate) fn fn_is_registered(scope: &Scope<'_>, keyword: &str) -> bool {
    scope
        .bindings()
        .iter_functions()
        .into_iter()
        .any(|(_, bucket)| {
            bucket.iter().any(|f| {
                f.signature.elements.iter().find_map(|e| match e {
                    SignatureElement::Keyword(s) => Some(s.as_str()),
                    _ => None,
                }) == Some(keyword)
            })
        })
}

/// Allocate a labeled marker object on `scope`'s arena. Dispatch tests register builtins
/// whose bodies return distinct markers so the test can assert which overload won.
pub(crate) fn marker<'a>(scope: &'a Scope<'a>, label: &'static str) -> &'a KObject<'a> {
    scope.arena.alloc_object(KObject::KString(label.into()))
}

/// Build a one-argument signature (`<name: kt>`) returning `Any`.
pub(crate) fn one_slot_sig<'a>(name: &str, kt: KType<'a>) -> ExpressionSignature<'a> {
    ExpressionSignature {
        return_type: ReturnType::Resolved(KType::Any),
        elements: vec![SignatureElement::Argument(Argument {
            name: name.into(),
            ktype: kt,
        })],
    }
}
