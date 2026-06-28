//! Shared test-only scaffolding for the builtin tests: PRINT-capturing `Write` sink,
//! parse/run/run_err harness over the dispatcher, run-root scope constructors, and
//! dispatch-test signature/marker builders.

use std::cell::RefCell;
use std::io::Write;
use std::rc::Rc;

use crate::machine::core::kfunction::KFunction;
use crate::machine::core::FrameStorage;
use crate::machine::execute::KoanRuntime;
use crate::machine::model::ast::KExpression;
use crate::machine::model::types::{
    Argument, ExpressionSignature, KType, ReturnType, SignatureElement,
};
use crate::machine::model::values::CarriedFamily;
use crate::machine::model::{Carried, KObject, Parseable};
use crate::machine::{KError, Scope};
use crate::parse::parse;
use crate::scheduler::{reattach_with, NodeId};

use super::default_scope;

/// Extract a top-level terminal at the scope lifetime `'a`. The scheduler reads at its own `&self`
/// borrow; re-anchoring the read to `'a` — a safe `reattach_with` witnessed by the `scope.region`
/// borrow — is sound. A returned closure / module rides a bare borrow into its per-call region, so
/// (like the production drain) fold the slot's witness onto `scope`'s reach-set: the caller drops the
/// scheduler right after this returns, and `scope` outlives it, so its reach-set keeps every region
/// the result reaches alive. Test-only — production code reads at the scheduler borrow.
pub(crate) fn extract_terminal<'a>(
    sched: &KoanRuntime<'a>,
    scope: &'a Scope<'a>,
    id: NodeId,
) -> Carried<'a> {
    let value = reattach_with::<CarriedFamily, _>(sched.read(id), scope.region);
    scope.fold_reach(&sched.dep_witness(id));
    value
}

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
    run_storage: &'a Rc<FrameStorage>,
) -> (&'a Scope<'a>, Rc<RefCell<Vec<u8>>>) {
    let buf = Rc::new(RefCell::new(Vec::new()));
    let scope = default_scope(run_storage, Box::new(SharedBuf(buf.clone())));
    (scope, buf)
}

pub(crate) fn run_root_silent<'a>(run_storage: &'a Rc<FrameStorage>) -> &'a Scope<'a> {
    default_scope(run_storage, Box::new(std::io::sink()))
}

/// Run-root scope with no builtins registered, for tests that exercise scope machinery
/// directly. Built inside `run_storage` like every run root, so its `region_owner` resolves —
/// tests that drive dispatch (establishing a run frame via `ensure_run_frame`) work the same as
/// pure scope-machinery tests that never reach the escape path.
pub(crate) fn run_root_bare<'a>(run_storage: &'a Rc<FrameStorage>) -> &'a Scope<'a> {
    run_storage.region().alloc_scope(Scope::run_root(
        run_storage,
        None,
        Box::new(std::io::sink()),
    ))
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
    let mut sched = KoanRuntime::new();
    let id = sched.dispatch_in_scope(expr, scope);
    sched.execute().expect("scheduler should succeed");
    extract_terminal(&sched, scope, id).object()
}

/// Like [`run_one`] but for a type-producing expression: narrows the result's carrier to
/// its [`Carried::Type`] arm. Panics if the expression produced a runtime value instead.
pub(crate) fn run_one_type<'a>(scope: &'a Scope<'a>, expr: KExpression<'a>) -> &'a KType<'a> {
    let mut sched = KoanRuntime::new();
    let id = sched.dispatch_in_scope(expr, scope);
    sched.execute().expect("scheduler should succeed");
    match extract_terminal(&sched, scope, id) {
        Carried::Type(kt) => kt,
        Carried::Object(obj) => panic!("expected a type result, got value {}", obj.summarize()),
    }
}

/// Like [`run_one`] but returns the `KError` produced by the dispatched node.
pub(crate) fn run_one_err<'a>(scope: &'a Scope<'a>, expr: KExpression<'a>) -> KError {
    let mut sched = KoanRuntime::new();
    let id = sched.dispatch_in_scope(expr, scope);
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
    let mut sched = KoanRuntime::new();
    for expr in exprs {
        sched.dispatch_in_scope(expr, scope);
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

/// Allocate a labeled marker object on `scope`'s region. Dispatch tests register builtins
/// whose bodies return distinct markers so the test can assert which overload won.
pub(crate) fn marker<'a>(scope: &Scope<'a>, label: &'static str) -> &'a KObject<'a> {
    scope.region.alloc_object(KObject::KString(label.into()))
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
