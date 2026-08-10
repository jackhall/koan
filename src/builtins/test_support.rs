//! Shared test scaffolding: the [`TestRun`] bundle (a seeded run root plus the runtime whose
//! run frame owns the run's sole [`TypeRegistry`]), a PRINT-capturing `Write` sink, the
//! parse/run/run_err harness over the dispatcher, and dispatch-test signature/marker builders.
//!
//! [`TestRun`] and [`SharedBuf`] are compiled unconditionally so the integration tests in
//! `tests/` reach them; everything else is `#[cfg(test)]`.

use std::cell::RefCell;
use std::io::Write;
use std::rc::Rc;

#[cfg(test)]
#[cfg(test)]
use crate::machine::KFunction;
use crate::machine::KoanRuntime;
#[cfg(test)]
use crate::machine::SealedFunction;
use crate::machine::core::{ProgramBrand, ProgramStorage, RegionBrand};
#[cfg(test)]
use crate::machine::model::Carried;
use crate::machine::model::KExpression;
use crate::machine::model::KObject;
#[cfg(test)]
use crate::machine::model::Module;
use crate::machine::model::TypeRegistry;
#[cfg(test)]
use crate::machine::model::{Argument, KType, ReturnType, SignatureDraft, SignatureElement};
use crate::machine::{AdoptSeam, FrameStorage, KError, NameLookup, Scope};
#[cfg(test)]
use crate::machine::{BindingIndex, DeclarationSite, NodeHandle, RunId};
use crate::parse::parse;
use crate::scheduler::NodeId;
#[cfg(test)]
use crate::witnessed::{Sealed, Witnessed};

use super::unseeded_scopes;

/// Mint a test [`DeclarationSite`] with a fresh run and an explicit installing node and lexical
/// index — the fixture stand-in for the run-qualified handle a scheduler-driven binder threads. A
/// distinct `node` simulates a distinct declaration statement; reuse one returned site (not a
/// second call) to simulate a parallel finalize of a single declaration.
#[cfg(test)]
pub(crate) fn mock_declaration_site(node: usize, index: usize) -> DeclarationSite {
    DeclarationSite {
        node: NodeHandle {
            run: RunId::next(),
            node: NodeId(node),
        },
        index: BindingIndex::value(index),
    }
}

/// A seeded test run: the run-root child `Scope`, the runtime that owns the run frame, and that
/// frame's [`TypeRegistry`] — the only registry in the tree.
///
/// The constructor follows production order (`interpret`): allocate the scope pair, establish the
/// run frame, then seed the builtins **against the frame's own registry**, so every seeded type is
/// registered against the registry the run later answers from. Holding the runtime is what keeps
/// that true across successive `run`/`run_one` calls: they share the run frame, and with it the
/// registry. Scope-only tests take `scope` and ignore the rest.
pub struct TestRun<'a> {
    /// The storage this run's AST is parsed into, borrowed for at least the run's life exactly as
    /// production borrows it: declared before the run storage, so it outlives every node read out
    /// of it.
    pub program: &'a ProgramStorage,
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
    pub fn new(
        program: &'a ProgramStorage,
        run_storage: &'a Rc<FrameStorage>,
        out: Box<dyn Write>,
    ) -> Self {
        let (root, child) = unseeded_scopes(run_storage);
        let mut runtime = KoanRuntime::new(program.brand(), out);
        // The run frame adopts `child`, exactly as `interpret` does: dispatch targets it, and the
        // frame it mints carries the registry seeding needs.
        runtime.ensure_run_frame(child);
        let types = runtime
            .type_registry()
            .expect("run frame was just established");
        crate::machine::seed_run_root(root, &types);
        Self {
            program,
            scope: child,
            runtime,
            types,
        }
    }

    /// [`TestRun::new`] with `PRINT` output discarded.
    pub fn silent(program: &'a ProgramStorage, run_storage: &'a Rc<FrameStorage>) -> Self {
        Self::new(program, run_storage, Box::new(std::io::sink()))
    }

    /// [`TestRun::new`] with `PRINT` output mirrored into a buffer the caller reads back.
    pub fn with_buf(
        program: &'a ProgramStorage,
        run_storage: &'a Rc<FrameStorage>,
    ) -> (Self, Rc<RefCell<Vec<u8>>>) {
        let buf = Rc::new(RefCell::new(Vec::new()));
        let run = Self::new(program, run_storage, Box::new(SharedBuf(buf.clone())));
        (run, buf)
    }

    /// The run's registry as a plain reference — the `types` argument the type-system surface takes.
    pub fn types(&self) -> &TypeRegistry {
        &self.types
    }

    /// This run's own region brand, for a test that builds AST directly rather than parsing it.
    pub fn brand(&self) -> RegionBrand<'a> {
        self.scope.brand()
    }

    /// The brand of the storage this run's AST is parsed into — the door every `parse` in a test
    /// rides, as `interpret` rides program storage's.
    pub fn program_brand(&self) -> ProgramBrand<'a> {
        self.program.brand()
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
    id: NodeId,
) -> Carried<'a> {
    // The extraction copies the value into `scope`'s region, so the copied-adoption rule applies:
    // the producer frame materializes into the surviving arena only when the copy's borrows
    // genuinely reach it (a returned closure / module), never for a residence-only scalar. The
    // witness and its retained host travel together as the delivery envelope.
    let delivered = runtime
        .dep_delivered(id)
        .expect("terminal should be a value, not an error");
    // Reuse the production relocation: a value that would otherwise keep region storage behind — a
    // substrate carrier, a bare string — is totally rebuilt into `scope`'s region through the seam
    // copy verb, every other object's top node is cloned at the fold brand, a type crosses by
    // handle / clone.
    scope.adopt_carried(&delivered, crate::machine::AdoptSeam::ReHome)
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
/// directly. Built inside `run_storage` like every run root, so its region owner resolves —
/// tests that drive dispatch (establishing a run frame via `ensure_run_frame`) work the same as
/// pure scope-machinery tests that never reach the escape path.
/// A **per-call**-tier storage with no ancestor — the fixture stand-in for another call's frame
/// whenever a white-box reach test needs a foreign region a retention is allowed to pin. The
/// run-root tier is not interchangeable here: its region outlives the run, so nothing takes an
/// owning pin on it.
#[cfg(test)]
pub(crate) fn per_call_storage() -> Rc<FrameStorage> {
    crate::witnessed::RegionHost::fresh(None)
}

#[cfg(test)]
pub(crate) fn run_root_bare<'a>(run_storage: &'a Rc<FrameStorage>) -> &'a Scope<'a> {
    Scope::alloc_run_root(run_storage)
}

/// Parse a source string expected to contain exactly one top-level expression into `program`,
/// which the caller declares ahead of its run storage so the node outlives every reader.
#[cfg(test)]
pub(crate) fn parse_one<'a>(program: &'a ProgramStorage, src: &str) -> KExpression<'a> {
    let mut exprs = parse(program.brand(), src).expect("parse should succeed");
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
        let exprs = parse(self.program_brand(), source).expect("parse should succeed");
        for expr in exprs {
            self.runtime.dispatch_in_scope(
                crate::machine::model::WorkingExpression::from_ast(scope.brand(), expr),
                scope,
            );
        }
        self.runtime.execute().expect("scheduler should succeed");
    }

    /// [`TestRun::run_in`] against the bundle's own scope.
    pub fn run(&mut self, source: &str) {
        self.run_in(self.scope, source)
    }

    /// Submit `source` as one block against `scope`, so its statements share a submission and
    /// resolve in whatever order the scheduler pops them — the shape a test asserting statement
    /// *ordering* needs, where [`TestRun::run_in`]'s statement-at-a-time dispatch would not.
    /// Returns the top-level node ids; the caller drives `execute` itself.
    pub fn enter_source_in(&mut self, scope: &'a Scope<'a>, source: &str) -> Vec<NodeId> {
        let statements = parse(self.program_brand(), source)
            .expect("parse should succeed")
            .into_iter()
            .map(|statement| {
                crate::machine::model::WorkingExpression::from_ast(scope.brand(), statement)
            })
            .collect();
        self.runtime.enter_block(scope.id, statements, scope)
    }

    /// [`TestRun::enter_source_in`] against the bundle's own scope.
    pub fn enter_source(&mut self, source: &str) -> Vec<NodeId> {
        self.enter_source_in(self.scope, source)
    }

    /// Parse `source` and dispatch each statement as its own submission against `scope`, handing
    /// back one node id per statement. The statement-at-a-time peer of
    /// [`TestRun::enter_source_in`], for a test that reads each slot's own result; the caller
    /// drives `execute` itself.
    pub fn dispatch_source_in(&mut self, scope: &'a Scope<'a>, source: &str) -> Vec<NodeId> {
        parse(self.program_brand(), source)
            .expect("parse should succeed")
            .into_iter()
            .map(|statement| {
                let working =
                    crate::machine::model::WorkingExpression::from_ast(scope.brand(), statement);
                self.runtime.dispatch_in_scope(working, scope)
            })
            .collect()
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
        let id = self.runtime.dispatch_in_scope(
            crate::machine::model::WorkingExpression::from_ast(scope.brand(), expr),
            scope,
        );
        self.runtime.execute().expect("scheduler should succeed");
        extract_terminal(&self.runtime, scope, id).object()
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
        let id = self.runtime.dispatch_in_scope(
            crate::machine::model::WorkingExpression::from_ast(scope.brand(), expr),
            scope,
        );
        self.runtime.execute().expect("scheduler should succeed");
        match extract_terminal(&self.runtime, scope, id) {
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
        let id = self.runtime.dispatch_in_scope(
            crate::machine::model::WorkingExpression::from_ast(scope.brand(), expr),
            scope,
        );
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

/// The value `name` binds to, **adopted** into `scope`'s own region so the reference outlives the
/// read — the assertion form of a binding read. Collapses a parked producer and a miss to `None`.
///
/// The bare `&KObject` shape lives here rather than on [`Scope`] because adoption is its price:
/// every production read that only *inspects* a binding goes through the delivery envelope and
/// retains nothing. `Scope`'s own bare ladder is `#[cfg(test)]`, which the integration tests in
/// `tests/` cannot see, so they reach the shape through this scaffolding module instead — and
/// production code, which never constructs a [`TestRun`], cannot reach it at all.
pub fn lookup_binding<'a>(scope: &Scope<'a>, name: &str) -> Option<&'a KObject<'a>> {
    scope
        .resolve_value_delivered(name, None)
        .and_then(NameLookup::bound)
        .map(|delivered| {
            scope
                .adopt_carried(&delivered, AdoptSeam::Retaining)
                .object()
        })
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
        for sealed in bucket {
            if first_keyword_of(scope, &sealed).as_deref() != Some(keyword) {
                continue;
            }
            assert!(
                found.is_none(),
                "ambiguous: multiple overloads under `{keyword}`"
            );
            found = Some(scope.open_function(&sealed).value());
        }
    }
    found.unwrap_or_else(|| panic!("no FN overload registered under `{keyword}`"))
}

/// The first keyword of a dormant overload's signature, read under `scope`'s own pin.
#[cfg(test)]
fn first_keyword_of(scope: &Scope<'_>, sealed: &SealedFunction) -> Option<String> {
    scope.read_function(sealed, |f| {
        f.signature.elements().iter().find_map(|e| match e {
            SignatureElement::Keyword(s) => Some((*s).to_string()),
            _ => None,
        })
    })
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
            bucket
                .iter()
                .any(|sealed| first_keyword_of(scope, sealed).as_deref() == Some(keyword))
        })
}

/// Allocate a labeled marker object on `scope`'s region. Dispatch tests register builtins
/// whose bodies return distinct markers so the test can assert which overload won.
#[cfg(test)]
pub(crate) fn marker<'a>(scope: &Scope<'a>, label: &'static str) -> &'a KObject<'a> {
    scope.brand().alloc_string(label)
}

/// The region-pure carrier a synthetic terminal needs: a description hosted in `scope`'s own region
/// naming no members — exactly what a value allocated straight into that region carries.
/// `Scheduler::set_result` writes a terminal onto a slot without running finalize, so the carrier
/// the real finalize would have minted has to be handed in here.
#[cfg(test)]
pub(crate) fn resident_carrier(scope: &Scope<'_>) -> crate::machine::CarrierWitness {
    crate::machine::CarrierWitness::new(scope.mint_retained(&[]))
}

/// Seal a resolved value into a region-pure `WorkingPart::Spliced` cell — the test-side peer of
/// the scheduler's splice, so a classification test can build the exact carrier a real splice rests
/// on the working expression. `Witnessed::resident_in` asserts the empty reach: the value borrows
/// only caller-held test data, not a foreign region.
///
/// `host` is the storage the description is minted into, and it must outlive every read of the
/// returned part: a resting cell owns no pin, so `host` plays the role the region a real splice
/// rests into plays in production. Borrowed at `'a` so that is a compile error rather than a rule —
/// every call site already passes the storage its `Carried` was allocated into, which is exactly
/// what production does.
#[cfg(test)]
pub(crate) fn spliced_part<'a>(
    host: &'a Rc<FrameStorage>,
    c: Carried<'a>,
) -> crate::machine::model::WorkingPart<'a> {
    crate::machine::model::WorkingPart::Spliced {
        cell: Sealed::seal(Witnessed::resident_in(c, host)),
    }
}

/// Build a one-argument signature (`<name: kt>`) returning `Any`.
#[cfg(test)]
pub(crate) fn one_slot_sig<'a>(name: &'a str, kt: KType) -> SignatureDraft<'a> {
    SignatureDraft {
        return_type: ReturnType::Resolved(KType::ANY),
        elements: vec![SignatureElement::Argument(Argument { name, ktype: kt })],
    }
}
