//! Shared test scaffolding: the [`TestRun`] bundle (a seeded run root plus the runtime whose
//! run frame owns the run's sole [`TypeRegistry`]), a PRINT-capturing `Write` sink, the
//! parse/run/run_err harness over the dispatcher, and dispatch-test signature/marker builders.
//!
//! [`TestRun`] and [`SharedBuf`] are compiled unconditionally so the integration tests in
//! `tests/` reach them; everything else is `#[cfg(test)]`.

use std::cell::RefCell;
use std::io::Write;
use std::rc::Rc;

use crate::machine::model::KExpression;
#[cfg(test)]
use crate::machine::model::Module;
use crate::machine::model::TypeRegistry;
#[cfg(test)]
use crate::machine::model::{Argument, ExpressionSignature, KType, ReturnType, SignatureElement};
#[cfg(test)]
use crate::machine::model::{Carried, ExpressionPart, KObject};
#[cfg(test)]
use crate::machine::FrameStorageExt;
use crate::machine::KoanRuntime;
#[cfg(test)]
use crate::machine::{DeliveredCarried, KFunction};
use crate::machine::{FrameStorage, KError, Scope};
use crate::parse::parse;
#[cfg(test)]
use crate::scheduler::NodeId;
#[cfg(test)]
use crate::witnessed::{Delivered, Sealed, Witnessed};

use super::{seed_builtins, unseeded_scopes};

/// A seeded test run: the run-root child `Scope`, the runtime that owns the run frame, and that
/// frame's [`TypeRegistry`] — the only registry in the tree.
///
/// The constructor follows production order (`interpret`): allocate the scope pair, establish the
/// run frame, then seed the builtins **against the frame's own registry**, so every seeded type is
/// registered against the registry the run later answers from. Holding the runtime is what keeps
/// that true across successive `run`/`run_one` calls: they share the run frame, and with it the
/// registry. Scope-only tests take `scope` and ignore the rest.
pub struct TestRun<'a> {
    /// The `RunScope` child of the seeded run root — the dispatch target.
    pub scope: &'a Scope<'a>,
    /// The runtime holding the run frame. Tests that drive the scheduler directly use it in place
    /// of a `KoanRuntime::new()` of their own.
    pub runtime: KoanRuntime<'a>,
    /// The run frame's registry, cloned out so it stays readable after the runtime drops.
    pub types: Rc<TypeRegistry>,
}

impl<'a> TestRun<'a> {
    /// Seed a run root inside `run_storage`, sending `PRINT` output to `out`.
    pub fn new(run_storage: &'a Rc<FrameStorage>, out: Box<dyn Write + 'a>) -> Self {
        let (root, child) = unseeded_scopes(run_storage, out);
        let mut runtime = KoanRuntime::new();
        // The run frame adopts `child`, exactly as `interpret` does: dispatch targets it, and the
        // frame it mints carries the registry seeding needs.
        runtime.ensure_run_frame(child);
        let types = runtime
            .type_registry()
            .expect("run frame was just established");
        seed_builtins(root, &types);
        Self {
            scope: child,
            runtime,
            types,
        }
    }

    /// [`TestRun::new`] with `PRINT` output discarded.
    pub fn silent(run_storage: &'a Rc<FrameStorage>) -> Self {
        Self::new(run_storage, Box::new(std::io::sink()))
    }

    /// [`TestRun::new`] with `PRINT` output mirrored into a buffer the caller reads back.
    pub fn with_buf(run_storage: &'a Rc<FrameStorage>) -> (Self, Rc<RefCell<Vec<u8>>>) {
        let buf = Rc::new(RefCell::new(Vec::new()));
        let run = Self::new(run_storage, Box::new(SharedBuf(buf.clone())));
        (run, buf)
    }

    /// The run's registry as a plain reference — the `types` argument the type-system surface takes.
    pub fn types(&self) -> &TypeRegistry {
        &self.types
    }
}

/// Extract a top-level terminal at the scope lifetime `'a`. The terminal is opened at a rank-2 brand
/// and its value **copied out** into `scope`'s region through the brand — a deep clone re-homed at
/// `'a` (the same copy a witnessed transfer's fold runs across a dep edge), so nothing branded
/// escapes the open. A returned closure / module's deep clone preserves the bare
/// borrow into its per-call region, so (like the production drain) fold the slot's witness onto
/// `scope`'s reach-set: the caller drops the scheduler right after this returns, and `scope` outlives
/// it, so its reach-set keeps every region the result reaches alive. Test-only — production code reads
/// inside the open without a fixed escape lifetime.
#[cfg(test)]
pub(crate) fn extract_terminal<'a>(
    runtime: &KoanRuntime<'a>,
    scope: &'a Scope<'a>,
    types: &TypeRegistry,
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
                    .alloc_object_delivered(obj.deep_clone(), std::slice::from_ref(&reach), types)
                    .expect("terminal object must be covered by its own stored reach"),
            ),
            // A type is a `Copy` handle: it rides across into `scope`'s region by value, naming no
            // reach. An unlowered type name crosses by clone through the single storage door.
            Carried::Type(kt) => Carried::Type(kt),
            Carried::UnresolvedType(ti) => {
                Carried::UnresolvedType(scope.brand().alloc_type_identifier(ti.clone()))
            }
        })
        .expect("terminal should be a value, not an error")
}

/// `Write` adapter that mirrors output into a shared `Vec<u8>` so tests can read it back.
pub struct SharedBuf(pub Rc<RefCell<Vec<u8>>>);

impl Write for SharedBuf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Run-root scope with no builtins registered, for tests that exercise scope machinery
/// directly. Built inside `run_storage` like every run root, so its `region_owner` resolves —
/// tests that drive dispatch (establishing a run frame via `ensure_run_frame`) work the same as
/// pure scope-machinery tests that never reach the escape path.
#[cfg(test)]
pub(crate) fn run_root_bare<'a>(run_storage: &'a Rc<FrameStorage>) -> &'a Scope<'a> {
    run_storage.brand().alloc_scope(Scope::run_root(
        run_storage,
        None,
        Box::new(std::io::sink()),
    ))
}

/// Parse a source string expected to contain exactly one top-level expression.
#[cfg(test)]
pub(crate) fn parse_one<'a>(src: &str) -> KExpression<'a> {
    let mut exprs = parse(src).expect("parse should succeed");
    assert_eq!(exprs.len(), 1, "test helper expects a single expression");
    exprs.remove(0)
}

/// The dispatch harness. Every method drives the bundle's own runtime, so successive calls share
/// the run frame — and with it the run's single [`TypeRegistry`], the one the builtins were seeded
/// against. The `_in` forms target a scope other than the bundle's own (a synthetic child, a
/// `SIG` body scope); the short forms target [`TestRun::scope`].
impl<'a> TestRun<'a> {
    /// REPL-style setup: parse `source` and dispatch each top-level statement against `scope`
    /// individually, so chained calls compose. Tests asserting top-level statement *ordering*
    /// (e.g. forward-ref-fails behavior) call `enter_block` on `runtime` directly instead.
    pub fn run_in(&mut self, scope: &'a Scope<'a>, source: &str) {
        let exprs = parse(source).expect("parse should succeed");
        for expr in exprs {
            self.runtime.dispatch_in_scope(expr, scope);
        }
        self.runtime.execute().expect("scheduler should succeed");
    }

    /// [`TestRun::run_in`] against the bundle's own scope.
    pub fn run(&mut self, source: &str) {
        self.run_in(self.scope, source)
    }

    /// Dispatch `expr` against `scope` with REPL-style "complete" visibility, so bindings from
    /// prior `run(...)` calls read through. Semantic errors surface via `read_result`, not
    /// `execute` — use [`TestRun::run_one_err`] when the test expects a `KError`.
    #[cfg(test)]
    pub(crate) fn run_one_in(
        &mut self,
        scope: &'a Scope<'a>,
        expr: KExpression<'a>,
    ) -> &'a KObject<'a> {
        let id = self.runtime.dispatch_in_scope(expr, scope);
        self.runtime.execute().expect("scheduler should succeed");
        extract_terminal(&self.runtime, scope, &self.types, id).object()
    }

    /// [`TestRun::run_one_in`] against the bundle's own scope.
    #[cfg(test)]
    pub(crate) fn run_one(&mut self, expr: KExpression<'a>) -> &'a KObject<'a> {
        self.run_one_in(self.scope, expr)
    }

    /// Like [`TestRun::run_one_in`] but for a type-producing expression: narrows the result's
    /// carrier to its [`Carried::Type`] arm. Panics if the expression produced a runtime value.
    #[cfg(test)]
    pub(crate) fn run_one_type_in(&mut self, scope: &'a Scope<'a>, expr: KExpression<'a>) -> KType {
        let id = self.runtime.dispatch_in_scope(expr, scope);
        self.runtime.execute().expect("scheduler should succeed");
        match extract_terminal(&self.runtime, scope, &self.types, id) {
            Carried::Type(kt) => kt,
            Carried::Object(obj) => panic!(
                "expected a type result, got value {}",
                obj.summarize(&self.types)
            ),
            Carried::UnresolvedType(ti) => panic!(
                "expected a resolved type result, got the unlowered name {}",
                ti.render()
            ),
        }
    }

    /// [`TestRun::run_one_type_in`] against the bundle's own scope.
    #[cfg(test)]
    pub(crate) fn run_one_type(&mut self, expr: KExpression<'a>) -> KType {
        self.run_one_type_in(self.scope, expr)
    }

    /// Like [`TestRun::run_one_in`] but returns the `KError` produced by the dispatched node.
    pub fn run_one_err_in(&mut self, scope: &'a Scope<'a>, expr: KExpression<'a>) -> KError {
        let id = self.runtime.dispatch_in_scope(expr, scope);
        self.runtime
            .execute()
            .expect("scheduler should not surface errors directly");
        match self.runtime.result_error(id) {
            Ok(()) => panic!("expected error"),
            Err(e) => e.clone(),
        }
    }

    /// [`TestRun::run_one_err_in`] against the bundle's own scope.
    pub fn run_one_err(&mut self, expr: KExpression<'a>) -> KError {
        self.run_one_err_in(self.scope, expr)
    }

    /// Release the scheduler's slot store, keeping the run frame, its registry, and every binding
    /// already on the run root. A test that measures a program's own slot footprint
    /// (`runtime.len()`, a free-list high-water mark) or the release of a frame a drained terminal
    /// retains calls this after its setup phase, so the measurement starts from an empty store.
    #[cfg(test)]
    pub(crate) fn reset_slots(&mut self) {
        self.runtime.reset_slots();
    }

    /// Like [`TestRun::run`], but splits the source in two phases: `prelude` first, then `probe`.
    /// Returns the run's [`TypeRegistry`] together with its hit and miss counts as of the end of
    /// `prelude`, so a test can measure each counter's movement across `probe` alone rather than
    /// over the whole run.
    #[cfg(test)]
    pub(crate) fn run_probe_returning_registry(
        &mut self,
        prelude: &str,
        probe: &str,
    ) -> (Rc<TypeRegistry>, usize, usize) {
        self.run(prelude);
        let registry = Rc::clone(&self.types);
        let hits_before_probe = registry.hit_count();
        let misses_before_probe = registry.miss_count();
        self.run(probe);
        (registry, hits_before_probe, misses_before_probe)
    }
}

/// The module `name` binds to. Modules are values, so the binding lives on the value channel
/// (`bindings.data`) and reads back as the Object-arm module value. Panics when `name` is unbound
/// or binds a non-module.
#[cfg(test)]
pub(crate) fn lookup_module<'a>(
    scope: &'a Scope<'a>,
    name: &str,
    types: &TypeRegistry,
) -> &'a Module<'a> {
    match scope.lookup(name) {
        Some(KObject::Module(module)) => module,
        other => panic!(
            "expected `{name}` to bind a module value in data, got {:?}",
            other.map(|o| o.ktype().name(types)),
        ),
    }
}

/// Whether `name` binds a module value — the predicate form of [`lookup_module`].
#[cfg(test)]
pub(crate) fn binds_module(scope: &Scope<'_>, name: &str) -> bool {
    matches!(scope.lookup(name), Some(KObject::Module(_)))
}

/// Fetch the single bare-`FN` overload whose signature's first keyword is `keyword`.
/// Panics if zero or more than one match.
#[cfg(test)]
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
#[cfg(test)]
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
#[cfg(test)]
pub(crate) fn marker<'a>(scope: &Scope<'a>, label: &'static str) -> &'a KObject<'a> {
    scope.brand().alloc_object(KObject::KString(label.into()))
}

/// Seal a resolved value into a region-pure `ExpressionPart::Spliced` cell — the test-side peer of
/// the scheduler's splice, so a classification test can build the exact carrier a real splice rests
/// on the working expression. `Witnessed::resident` asserts the empty reach: the value borrows only
/// caller-held test data, not a foreign region — a fresh throwaway storage stands in as the
/// envelope's host pin.
#[cfg(test)]
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
#[cfg(test)]
pub(crate) fn delivered_with_host(value: Carried<'_>, host: Rc<FrameStorage>) -> DeliveredCarried {
    Delivered::seal(Witnessed::resident(value), host)
}

/// Build a one-argument signature (`<name: kt>`) returning `Any`.
#[cfg(test)]
pub(crate) fn one_slot_sig<'a>(name: &str, kt: KType) -> ExpressionSignature<'a> {
    ExpressionSignature {
        return_type: ReturnType::Resolved(KType::ANY),
        elements: vec![SignatureElement::Argument(Argument {
            name: name.into(),
            ktype: kt,
        })],
    }
}
