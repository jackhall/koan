//! Shared test-only scaffolding for the builtin tests: PRINT-capturing `Write` sink,
//! parse/run/run_err harness over the dispatcher, run-root scope constructors, and
//! dispatch-test signature/marker builders.

use std::cell::RefCell;
use std::io::Write;
use std::rc::Rc;

use crate::machine::model::Module;
use crate::machine::model::TypeRegistry;
use crate::machine::model::{Argument, ExpressionSignature, KType, ReturnType, SignatureElement};
use crate::machine::model::{Carried, KObject};
use crate::machine::model::{ExpressionPart, KExpression};
use crate::machine::KFunction;
use crate::machine::KoanRuntime;
use crate::machine::{DeliveredCarried, KError, Scope};
use crate::machine::{FrameStorage, FrameStorageExt};
use crate::parse::parse;
use crate::scheduler::NodeId;
use crate::witnessed::{Delivered, Sealed, Witnessed};

use super::default_scope;

/// Extract a top-level terminal at the scope lifetime `'a`. The terminal is opened at a rank-2 brand
/// and its value **copied out** into `scope`'s region through the brand — a deep clone re-homed at
/// `'a` (the same copy a witnessed transfer's fold runs across a dep edge), so nothing branded
/// escapes the open. A returned closure / module's deep clone preserves the bare
/// borrow into its per-call region, so (like the production drain) fold the slot's witness onto
/// `scope`'s reach-set: the caller drops the scheduler right after this returns, and `scope` outlives
/// it, so its reach-set keeps every region the result reaches alive. Test-only — production code reads
/// inside the open without a fixed escape lifetime.
pub(crate) fn extract_terminal<'a>(
    runtime: &KoanRuntime<'a>,
    scope: &'a Scope<'a>,
    id: NodeId,
) -> Carried<'a> {
    // The extraction deep-clones the value into `scope`'s region, so the copied-adoption rule
    // applies: the producer frame materializes into the surviving arena only when the copy's
    // borrows genuinely reach it (a returned closure / module), never for a residence-only scalar.
    // The witness and its retained host travel together as the delivery envelope. Minted *before*
    // the read below so the deep-cloned copy's own residence audit can see it — a returned
    // closure / module's deep clone preserves the bare borrow into its per-call region.
    let delivered = runtime
        .dep_delivered(id)
        .expect("terminal should be a value, not an error");
    let reach = scope.adopted_reach_of(&delivered);
    runtime
        .read_result_with(id, |live| match live {
            Carried::Object(obj) => Carried::Object(
                scope
                    .alloc_object_delivered(
                        obj.deep_clone(),
                        std::slice::from_ref(&reach),
                        &TypeRegistry::new(),
                    )
                    .expect("terminal object must be covered by its own stored reach"),
            ),
            // A type is owned data: it crosses into `scope`'s region by clone through the single
            // storage door, naming no reach. An unlowered type name crosses the same way.
            Carried::Type(kt) => Carried::Type(scope.brand().alloc_ktype(kt.clone())),
            Carried::UnresolvedType(ti) => {
                Carried::UnresolvedType(scope.brand().alloc_type_identifier(ti.clone()))
            }
        })
        .expect("terminal should be a value, not an error")
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
    run_storage.brand().alloc_scope(Scope::run_root(
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
    let mut runtime = KoanRuntime::new();
    let id = runtime.dispatch_in_scope(expr, scope);
    runtime.execute().expect("scheduler should succeed");
    extract_terminal(&runtime, scope, id).object()
}

/// Like [`run_one`] but for a type-producing expression: narrows the result's carrier to
/// its [`Carried::Type`] arm. Panics if the expression produced a runtime value instead.
pub(crate) fn run_one_type<'a>(scope: &'a Scope<'a>, expr: KExpression<'a>) -> &'a KType {
    let mut runtime = KoanRuntime::new();
    let id = runtime.dispatch_in_scope(expr, scope);
    runtime.execute().expect("scheduler should succeed");
    match extract_terminal(&runtime, scope, id) {
        Carried::Type(kt) => kt,
        Carried::Object(obj) => panic!(
            "expected a type result, got value {}",
            obj.summarize(&TypeRegistry::new())
        ),
        Carried::UnresolvedType(ti) => {
            panic!(
                "expected a resolved type result, got the unlowered name {}",
                ti.render()
            )
        }
    }
}

/// Like [`run_one`] but returns the `KError` produced by the dispatched node.
pub(crate) fn run_one_err<'a>(scope: &'a Scope<'a>, expr: KExpression<'a>) -> KError {
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

/// REPL-style setup: parse `source` and dispatch each top-level statement individually,
/// so chained `run(scope, ...)` calls compose. Tests asserting top-level statement
/// *ordering* (e.g. forward-ref-fails behavior) call `enter_block` directly instead.
pub(crate) fn run<'a>(scope: &'a Scope<'a>, source: &str) {
    let mut runtime = KoanRuntime::new();
    dispatch_phase(&mut runtime, scope, source);
}

/// Parse `source` and dispatch each top-level statement against `runtime`, then drain the
/// scheduler. Runs one phase of work on a runtime the caller retains, so successive phases share
/// the run frame — and with it the run-scoped [`TypeRegistry`].
fn dispatch_phase<'a>(runtime: &mut KoanRuntime<'a>, scope: &'a Scope<'a>, source: &str) {
    let exprs = parse(source).expect("parse should succeed");
    for expr in exprs {
        runtime.dispatch_in_scope(expr, scope);
    }
    runtime.execute().expect("scheduler should succeed");
}

/// Like [`run`], but splits the source in two phases run against one retained
/// runtime: `prelude` first, then `probe`. Returns the run's [`TypeRegistry`] together with its
/// hit and miss counts as of the end of `prelude`, so a test can measure each counter's movement
/// across `probe` alone rather than over the whole run. Verdicts are run-scoped, so a single
/// runtime is what makes the two phases share a registry at all; the registry dies with the
/// runtime, so the `Rc` is cloned out before the runtime drops.
pub(crate) fn run_probe_returning_registry<'a>(
    scope: &'a Scope<'a>,
    prelude: &str,
    probe: &str,
) -> (Rc<TypeRegistry>, usize, usize) {
    let mut runtime = KoanRuntime::new();
    dispatch_phase(&mut runtime, scope, prelude);
    let registry = runtime
        .type_registry()
        .expect("a dispatched run establishes the run frame and its registry");
    let hits_before_probe = registry.hit_count();
    let misses_before_probe = registry.miss_count();
    dispatch_phase(&mut runtime, scope, probe);
    (registry, hits_before_probe, misses_before_probe)
}

/// The module `name` binds to. Modules are values, so the binding lives on the value channel
/// (`bindings.data`) and reads back as the Object-arm module value. Panics when `name` is unbound
/// or binds a non-module.
pub(crate) fn lookup_module<'a>(scope: &'a Scope<'a>, name: &str) -> &'a Module<'a> {
    match scope.lookup(name) {
        Some(KObject::Module(module)) => module,
        other => panic!(
            "expected `{name}` to bind a module value in data, got {:?}",
            other.map(|o| o.ktype().name(&TypeRegistry::new())),
        ),
    }
}

/// Whether `name` binds a module value — the predicate form of [`lookup_module`].
pub(crate) fn binds_module(scope: &Scope<'_>, name: &str) -> bool {
    matches!(scope.lookup(name), Some(KObject::Module(_)))
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
    scope.brand().alloc_object(KObject::KString(label.into()))
}

/// Seal a resolved value into a region-pure `ExpressionPart::Spliced` cell — the test-side peer of
/// the scheduler's splice, so a classification test can build the exact carrier a real splice rests
/// on the working expression. `Witnessed::resident` asserts the empty reach: the value borrows only
/// caller-held test data, not a foreign region — a fresh throwaway storage stands in as the
/// envelope's host pin.
pub(crate) fn spliced_part(c: Carried<'_>) -> ExpressionPart<'_> {
    ExpressionPart::Spliced {
        cell: Delivered::hosted(
            Sealed::seal(Witnessed::resident(c)),
            crate::machine::run_root_storage(),
        ),
    }
}

/// Build a delivery envelope around `value` (an empty-reach resident witness) pinned by `host` —
/// for tests that only need a real `DeliveredCarried` to drive a mint, not any particular reach
/// content. `Delivered::seal` requires the true owner in hand, so the caller supplies `host`
/// exactly as a scheduler pull or a resident seal would.
pub(crate) fn delivered_with_host(value: Carried<'_>, host: Rc<FrameStorage>) -> DeliveredCarried {
    Delivered::seal(Witnessed::resident(value), host)
}

/// Build a one-argument signature (`<name: kt>`) returning `Any`.
pub(crate) fn one_slot_sig<'a>(name: &str, kt: KType) -> ExpressionSignature<'a> {
    ExpressionSignature {
        return_type: ReturnType::Resolved(KType::Any),
        elements: vec![SignatureElement::Argument(Argument {
            name: name.into(),
            ktype: kt,
        })],
    }
}
