//! Shared test-only scaffolding for the builtin tests: PRINT-capturing `Write` sink,
//! parse/run/run_err harness over the dispatcher, run-root scope constructors, and
//! dispatch-test signature/marker builders.

use std::cell::RefCell;
use std::io::Write;
use std::rc::Rc;

use crate::machine::core::kfunction::KFunction;
use crate::machine::core::{FrameStorage, FrameStorageExt};
use crate::machine::execute::KoanRuntime;
use crate::machine::model::ast::{ExpressionPart, KExpression};
use crate::machine::model::types::{
    Argument, ExpressionSignature, KType, ReturnType, SignatureElement,
};
use crate::machine::model::{Carried, KObject, Parseable};
use crate::machine::{DeliveredCarried, KError, Scope};
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
                    .alloc_object_delivered(obj.deep_clone(), std::slice::from_ref(&reach))
                    .expect("terminal object must be covered by its own stored reach"),
            ),
            Carried::Type(kt) => Carried::Type(
                scope
                    .alloc_ktype_reaching(kt.clone(), &reach)
                    .expect("terminal type must be covered by its own stored reach"),
            ),
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

/// Register a root-scope arity-1 type constructor named `name` (a real, non-sentinel
/// `TypeConstructor`, its single parameter named `Type`), so a koan `LET <Member> = <name>`
/// can supply a signature's higher-kinded abstract slot (`TYPE (Type AS <Member>)`). The only
/// builtin constructor, `Result`, is arity 2, so an arity-1 slot needs a minted one.
pub(crate) fn register_arity1_constructor<'a>(scope: &'a Scope<'a>, name: &str) {
    use crate::machine::model::types::{NominalSchema, RecursiveSet};
    use crate::machine::{BindingIndex, ScopeId};
    let set = RecursiveSet::singleton(
        name.into(),
        ScopeId::from_raw(0, 0xC0DE),
        NominalSchema::TypeConstructor {
            schema: std::collections::HashMap::new(),
            param_names: vec!["Type".into()],
        },
    );
    scope.register_builtin_type(
        name.into(),
        KType::SetRef { set, index: 0 },
        BindingIndex::BUILTIN,
    );
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
pub(crate) fn run_one_type<'a>(scope: &'a Scope<'a>, expr: KExpression<'a>) -> &'a KType<'a> {
    let mut runtime = KoanRuntime::new();
    let id = runtime.dispatch_in_scope(expr, scope);
    runtime.execute().expect("scheduler should succeed");
    match extract_terminal(&runtime, scope, id) {
        Carried::Type(kt) => kt,
        Carried::Object(obj) => panic!("expected a type result, got value {}", obj.summarize()),
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
    let exprs = parse(source).expect("parse should succeed");
    let mut runtime = KoanRuntime::new();
    for expr in exprs {
        runtime.dispatch_in_scope(expr, scope);
    }
    runtime.execute().expect("scheduler should succeed");
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
            crate::machine::core::run_root_storage(),
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
pub(crate) fn one_slot_sig<'a>(name: &str, kt: KType<'a>) -> ExpressionSignature<'a> {
    ExpressionSignature {
        return_type: ReturnType::Resolved(KType::Any),
        elements: vec![SignatureElement::Argument(Argument {
            name: name.into(),
            ktype: kt,
        })],
    }
}
