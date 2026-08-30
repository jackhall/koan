//! Shared test scaffolding: the [`TestRun`] bundle (a seeded run root plus the runtime whose
//! run frame owns the run's sole [`TypeRegistry`]), a PRINT-capturing `Write` sink, the
//! parse/run/run_err harness over the dispatcher, and dispatch-test signature/marker builders.
//!
//! [`TestRun`] and [`SharedBuf`] are compiled unconditionally so the integration tests in
//! `tests/` reach them; everything else is `#[cfg(test)]`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Write;
use std::rc::Rc;

#[cfg(test)]
use crate::machine::KFunction;
use crate::machine::KoanRuntime;
use crate::machine::ScopeId;
#[cfg(test)]
use crate::machine::SealedFunction;
use crate::machine::core::CallFrame;
#[cfg(test)]
use crate::machine::core::StatementId;
use crate::machine::core::{ProgramBrand, ProgramStorage, RegionBrand};
#[cfg(test)]
use crate::machine::model::Carried;
#[cfg(test)]
use crate::machine::model::ExpressionPart;
use crate::machine::model::KExpression;
use crate::machine::model::KObject;
#[cfg(test)]
use crate::machine::model::Module;
use crate::machine::model::RunRegistries;
use crate::machine::model::TypeRegistry;
#[cfg(test)]
use crate::machine::model::{Argument, KType, ReturnType, SignatureDraft, SignatureElement};
use crate::machine::{AdoptSeam, FrameStorage, KError, NameLookup, Scope};
#[cfg(test)]
use crate::machine::{BindingIndex, DeclarationSite, Installer};
use crate::parse::parse;
use crate::scheduler::{EdgeId, NodeId};
#[cfg(test)]
use crate::witnessed::{RegionHandle, Sealed};

use super::unseeded_scopes;

/// Mint a test [`DeclarationSite`] at lexical position `index`, under a freshly minted
/// [`StatementId`] — the fixture stand-in for the identity a submitted binder threads. Every call
/// names a *different* declaration statement; reuse one returned site (not a second call) to
/// simulate a parallel finalize of a single declaration.
#[cfg(test)]
pub(crate) fn mock_declaration_site(index: usize) -> DeclarationSite {
    DeclarationSite {
        installer: Installer::Statement(StatementId::next()),
        index: BindingIndex::value(index),
    }
}

/// A fixture's value-token name as the [`ValueSymbol`] a binding door takes, interned so a
/// diagnostic naming it renders the text back. Panics on a name of the wrong token class — a
/// fixture binding a value under a Type token is ill-formed.
#[cfg(test)]
pub(crate) fn value_name(
    text: &str,
    registries: &crate::machine::model::RunRegistries,
) -> crate::machine::model::ValueSymbol {
    crate::machine::model::ValueSymbol::declared(text, &registries.labels)
        .unwrap_or_else(|| panic!("test fixture name `{text}` is not a value token"))
}

/// [`value_name`] for the type channel.
#[cfg(test)]
pub(crate) fn type_name(
    text: &str,
    registries: &crate::machine::model::RunRegistries,
) -> crate::machine::model::TypeSymbol {
    crate::machine::model::TypeSymbol::declared(text, &registries.labels)
        .unwrap_or_else(|| panic!("test fixture name `{text}` is not a Type token"))
}

/// [`type_name`] without the interner: pure classification through the hidden funnel, for a
/// fixture that holds no [`RunRegistries`](crate::machine::model::RunRegistries) and asserts on
/// symbol identity rather than on rendered text. Production has no bare `TypeSymbol` probe — a
/// Type token is minted at the parse that classifies it.
#[cfg(test)]
pub(crate) fn type_token(text: &str) -> crate::machine::model::TypeSymbol {
    crate::machine::model::TypeSymbol::classify(text)
        .unwrap_or_else(|| panic!("test fixture name `{text}` is not a Type token"))
}

/// [`value_name`] for a seam that takes either bindable class.
#[cfg(test)]
pub(crate) fn binder_name(
    text: &str,
    registries: &crate::machine::model::RunRegistries,
) -> crate::machine::model::BinderSymbol {
    crate::machine::model::BinderSymbol::declared(text, &registries.labels)
        .unwrap_or_else(|| panic!("test fixture name `{text}` is not a bindable token"))
}

/// [`binder_name`] without the interner — [`type_token`]'s bindable-class twin, for a fixture
/// that keys a record or a signature by symbol identity and never renders the name.
#[cfg(test)]
pub(crate) fn binder_token(text: &str) -> crate::machine::model::BinderSymbol {
    crate::machine::model::BinderSymbol::classify(text)
        .unwrap_or_else(|| panic!("test fixture name `{text}` is not a bindable token"))
}

/// A seeded test run: the run-root child `Scope`, the runtime that owns the run frame, and that
/// frame's [`TypeRegistry`].
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
    /// The run frame, shared out so its registries stay readable after the runtime drops — and
    /// without borrowing the runtime, which every `run` call needs mutably.
    run_frame: Rc<CallFrame>,
    /// Per-scope statement cursors — the session's own record of how many top-level statements it
    /// has submitted against each scope, which is what numbers the next one. The lexical position
    /// of a statement-at-a-time submission exists only in the submitting session (statement N is
    /// the N-th line typed), so the driver carries it and hands it to the runtime explicitly;
    /// the runtime stores nothing. Starts at `1` per scope: builtins sit at index `0`.
    cursors: HashMap<ScopeId, usize>,
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
        let registries = runtime
            .registries()
            .expect("run frame was just established");
        crate::machine::seed_run_root(root, registries);
        let run_frame = runtime.run_frame().expect("run frame was just established");
        Self {
            program,
            scope: child,
            runtime,
            run_frame,
            cursors: HashMap::new(),
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

    /// The run frame held for its registries — what a test binds when it needs the registry
    /// across `run` calls, which borrow the `TestRun` mutably.
    pub fn registry_handle(&self) -> RegistryHandle {
        RegistryHandle(Rc::clone(&self.run_frame))
    }

    /// [`parse_one`] into this bundle's own program storage and interner — what production does,
    /// so a Type token this run declares is resolvable by every diagnostic it renders.
    #[cfg(test)]
    pub(crate) fn parse_one(&self, src: &str) -> KExpression<'a> {
        parse_one(self.program, &self.registries().labels, src)
    }

    /// The run's lookup state — the currency for anything that renders a label or builds a record.
    pub fn registries(&self) -> &RunRegistries {
        self.run_frame
            .registries()
            .expect("the run frame carries the registries")
    }

    /// The run's registry as a plain reference — the `types` argument the type-system surface takes.
    pub fn types(&self) -> &TypeRegistry {
        &self.registries().types
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

/// Extract a watched terminal at the scope lifetime `'a`. `edge` is the harness's own name for the
/// result — installed against `scope`'s region before the run, so the producer delivered there — and
/// what it holds is an ordinary resident of that region.
///
/// The resident is re-branded under the scope's own owner (the same brand the run loop mints for a
/// step's deps), lifted back into an envelope, and its value **copied out** into `scope`'s region: a
/// deep clone re-homed at `'a`, so nothing branded escapes the read. A returned closure / module's
/// deep clone preserves the bare borrow into its per-call region, so the copy's own reach is minted
/// into `scope`'s region — the caller drops the scheduler right after this returns, and `scope`
/// outlives it, so its reach-set keeps every region the result still reaches alive. Test-only —
/// production code reads inside the open without a fixed escape lifetime.
#[cfg(test)]
pub(crate) fn extract_terminal<'a>(
    runtime: &KoanRuntime<'a>,
    scope: &'a Scope<'a>,
    edge: EdgeId,
) -> Carried<'a> {
    let delivered = crate::machine::execute::edge_delivered(runtime, edge, scope)
        .expect("terminal should be a value, not an error");
    // Reuse the production relocation: a value that would otherwise keep region storage behind — a
    // substrate carrier, a bare string — is totally rebuilt into `scope`'s region through the seam
    // copy verb.
    scope.adopt_copied_for_test(&delivered)
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
///
/// `labels` is the interner the parse declares its Type tokens into. A fixture driving a run
/// passes that run's own — [`TestRun::parse_one`] does it for you — so a diagnostic naming one
/// resolves its spelling; a fixture asserting only on shape can pass a throwaway.
#[cfg(test)]
pub(crate) fn parse_one<'a>(
    program: &'a ProgramStorage,
    labels: &crate::machine::model::LabelInterner,
    src: &str,
) -> KExpression<'a> {
    let mut exprs = parse(program.brand(), labels, src).expect("parse should succeed");
    assert_eq!(exprs.len(), 1, "test helper expects a single expression");
    exprs.remove(0)
}

/// The dispatch harness. Every method drives the bundle's own runtime, so successive calls share
/// the run frame — and with it the run's single [`TypeRegistry`], the one the builtins were seeded
/// against. The `_in` forms target a scope other than the bundle's own (a synthetic child, a
/// `SIG` body scope); the short forms target [`TestRun::scope`].
impl<'a> TestRun<'a> {
    /// The next statement position on `scope`, advancing the session's cursor: the number this
    /// statement would carry as the next line of a file targeting that scope.
    pub fn next_statement_index(&mut self, scope: &Scope<'_>) -> usize {
        let cursor = self.cursors.entry(scope.id).or_insert(1);
        let index = *cursor;
        *cursor += 1;
        index
    }

    /// The current statement position on `scope` without advancing — where a read-only watcher
    /// submitted "after everything so far" sits.
    pub fn statement_index(&mut self, scope: &Scope<'_>) -> usize {
        *self.cursors.entry(scope.id).or_insert(1)
    }

    /// Advance `scope`'s cursor past a block of `count` statements entered positionally
    /// (`enter_block` numbers them `1..=count`), so a later statement-at-a-time submission
    /// numbers after them.
    fn skip_block_indices(&mut self, scope: &Scope<'_>, count: usize) {
        let cursor = self.cursors.entry(scope.id).or_insert(1);
        *cursor = (*cursor).max(count + 1);
    }

    /// Dispatch `working` against `scope` at the session cursor's position — the cursor-driven
    /// form of [`KoanRuntime::dispatch_in_scope`], for tests that want the raw `NodeId`.
    pub fn dispatch_in_scope(
        &mut self,
        working: crate::machine::model::WorkingExpression<'a>,
        scope: &'a Scope<'a>,
    ) -> NodeId {
        let index = self.next_statement_index(scope);
        self.runtime.dispatch_in_scope(working, scope, index)
    }

    /// REPL-style setup: parse `source` and dispatch each top-level statement against `scope`
    /// individually, so chained calls compose. Tests asserting top-level statement *ordering*
    /// (e.g. forward-ref-fails behavior) call `enter_block` on `runtime` directly instead.
    pub fn run_in(&mut self, scope: &'a Scope<'a>, source: &str) {
        let exprs = parse(self.program_brand(), &self.registries().labels, source)
            .expect("parse should succeed");
        for expr in exprs {
            self.dispatch_in_scope(
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
        let statements: Vec<crate::machine::model::WorkingExpression<'a>> =
            parse(self.program_brand(), &self.registries().labels, source)
                .expect("parse should succeed")
                .into_iter()
                .map(|statement| {
                    crate::machine::model::WorkingExpression::from_ast(scope.brand(), statement)
                })
                .collect();
        self.skip_block_indices(scope, statements.len());
        self.runtime
            .enter_block(scope.id, statements, scope)
            .into_iter()
            .collect()
    }

    /// [`TestRun::enter_source_in`] with a watch edge wired onto each statement's slot, so a test
    /// can read what each one produced. Block submission, so the statements sit at successive
    /// lexical positions on one chain — which is what the index-gated visibility rule reads.
    pub fn enter_source_watched_in(&mut self, scope: &'a Scope<'a>, source: &str) -> Vec<EdgeId> {
        self.enter_source_in(scope, source)
            .into_iter()
            .map(|id| self.runtime.install_edge_for_test(id, scope))
            .collect()
    }

    /// [`TestRun::enter_source_in`] against the bundle's own scope.
    pub fn enter_source(&mut self, source: &str) -> Vec<NodeId> {
        self.enter_source_in(self.scope, source)
    }

    /// Parse `source` and dispatch each statement as its own submission against `scope`, handing
    /// back one node id per statement. The statement-at-a-time peer of
    /// [`TestRun::enter_source_in`], for a test that reads each slot's own result; the caller
    /// drives `execute` itself.
    /// Every statement is submitted before any watch edge is wired, so the submissions land
    /// back to back — a claim a later statement installs is in place by the time an earlier one
    /// steps, which is the ordering the visibility rule is written against.
    pub fn dispatch_source_in(&mut self, scope: &'a Scope<'a>, source: &str) -> Vec<EdgeId> {
        let slots: Vec<NodeId> = parse(self.program_brand(), &self.registries().labels, source)
            .expect("parse should succeed")
            .into_iter()
            .map(|statement| {
                let working =
                    crate::machine::model::WorkingExpression::from_ast(scope.brand(), statement);
                self.dispatch_in_scope(working, scope)
            })
            .collect();
        slots
            .into_iter()
            .map(|id| self.runtime.install_edge_for_test(id, scope))
            .collect()
    }

    /// Dispatch `working` against `scope` and **watch** its result: the harness wires an edge onto
    /// the fresh slot, destined at `scope`'s own region, exactly as `run_program` wires the run's
    /// roots. Slots reclaim at finalize, so an edge is what a reader holds — a `NodeId` would name
    /// nothing by the time `execute` returns.
    pub fn dispatch_watched_in(
        &mut self,
        scope: &'a Scope<'a>,
        working: crate::machine::model::WorkingExpression<'a>,
    ) -> EdgeId {
        let id = self.dispatch_in_scope(working, scope);
        self.runtime.install_edge_for_test(id, scope)
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
        let edge = self.dispatch_watched_in(
            scope,
            crate::machine::model::WorkingExpression::from_ast(scope.brand(), expr),
        );
        self.runtime.execute().expect("scheduler should succeed");
        extract_terminal(&self.runtime, scope, edge).object()
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
        let edge = self.dispatch_watched_in(
            scope,
            crate::machine::model::WorkingExpression::from_ast(scope.brand(), expr),
        );
        self.runtime.execute().expect("scheduler should succeed");
        match extract_terminal(&self.runtime, scope, edge) {
            Carried::Type(kt) => kt,
            Carried::Object(obj) => panic!(
                "expected a type result, got value {}",
                obj.summary(self.registries())
            ),
            Carried::UnresolvedType(ti) => panic!(
                "expected a resolved type result, got the unlowered name {}",
                crate::machine::model::render_label(ti.symbol(), self.registries())
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
        let edge = self.dispatch_watched_in(
            scope,
            crate::machine::model::WorkingExpression::from_ast(scope.brand(), expr),
        );
        self.runtime
            .execute()
            .expect("scheduler should not surface errors directly");
        match self.runtime.edge_result_error(edge) {
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
    ) -> (RegistryHandle, usize, usize) {
        self.run(prelude);
        let registry = self.registry_handle();
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
/// The type `name` resolves to in `scope`'s type channel, looked up from surface text.
///
/// Production reaches [`Scope::resolve_type`] with a token the parser already classified and
/// interned; a test states its name as source text, so the classification happens here. A
/// non-Type-token spelling names nothing on this channel and answers `None`, which is the
/// disposition the `types` table would have given it anyway.
pub fn lookup_type(scope: &Scope<'_>, name: &str) -> Option<crate::machine::model::KType> {
    scope.resolve_type(crate::machine::model::TypeSymbol::classify(name)?)
}

pub fn lookup_binding<'a>(scope: &Scope<'a>, name: &str) -> Option<&'a KObject<'a>> {
    scope
        .resolve_value_delivered(crate::machine::model::ValueSymbol::classify(name)?, None)
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
    registries: &RunRegistries,
) -> &'a Module<'a> {
    match scope.lookup(name) {
        Some(KObject::Module(module)) => module,
        other => panic!(
            "expected `{name}` to bind a module value in data, got {:?}",
            other.map(|o| o.ktype().name(registries)),
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
            if first_keyword_of(scope, &sealed) != Some(probe_symbol(keyword)) {
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
fn first_keyword_of(
    scope: &Scope<'_>,
    sealed: &SealedFunction,
) -> Option<crate::machine::model::labels::KeywordSymbol> {
    scope.read_function(sealed, |f| {
        f.signature.elements().iter().find_map(|e| match e {
            SignatureElement::Keyword(symbol) => Some(*symbol),
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
                .any(|sealed| first_keyword_of(scope, sealed) == Some(probe_symbol(keyword)))
        })
}

/// A keyword part for a hand-built AST: classify and mint, recording nothing. A test node is not a
/// declaration, so nothing resolves its keywords through the run's interner.
#[cfg(test)]
pub(crate) fn kw_part<'a>(text: &str) -> ExpressionPart<'a> {
    ExpressionPart::Keyword(
        crate::machine::model::KeywordSymbol::of(text)
            .expect("a test fixture keyword is keyword-class"),
    )
}

/// [`kw_part`]'s value-channel twin: an identifier part for a hand-built AST, classified and minted
/// with nothing recorded. A test node is not a declaration, so nothing renders its names through
/// the run's interner; a fixture that does render one states its name through
/// [`value_name`] against the run's own registries instead.
#[cfg(test)]
pub(crate) fn identifier_part<'a>(text: &str) -> ExpressionPart<'a> {
    ExpressionPart::Identifier(
        crate::machine::model::ValueSymbol::classify(text)
            .expect("a test fixture identifier is a value token"),
    )
}

/// The operator-probe symbol for a probe key a test spells out (`"+"`, `"* +"`).
#[cfg(test)]
pub(crate) fn probe_symbol(text: &str) -> crate::machine::model::labels::KeywordSymbol {
    crate::machine::model::labels::KeywordSymbol::of(text)
        .expect("a test fixture operator probe is keyword-class")
}

/// The operator-registry probe key a run of `glyphs` stands for — the same run digest
/// `operator_probe_for` mints for a live chain and `powerset_probes` registers under, so a test
/// spelling a probe by hand keys exactly what the machine would. Declared, not probed: a fixture
/// that registers under this key stands in for a real registration, and a diagnostic naming the key
/// has to resolve it.
#[cfg(test)]
pub(crate) fn operator_run(
    glyphs: &[&str],
    registries: &crate::machine::model::RunRegistries,
) -> crate::machine::model::labels::KeywordSymbol {
    let members: Vec<_> = glyphs
        .iter()
        .map(|glyph| {
            crate::machine::model::labels::KeywordSymbol::declared(glyph, &registries.labels)
                .unwrap_or_else(|| panic!("test fixture glyph `{glyph}` is not keyword-class"))
        })
        .collect();
    crate::machine::model::labels::KeywordSymbol::declared_run(&members, &registries.labels)
}

/// A keyword bucket-key element from its spelling, for a key a test spells out by hand.
#[cfg(test)]
pub(crate) fn key_keyword(text: &str) -> crate::machine::model::KeyElement {
    crate::machine::model::KeyElement::Keyword(
        crate::machine::model::labels::KeywordSymbol::of(text)
            .expect("a test fixture keyword is keyword-class"),
    )
}

/// Allocate a labeled marker object on `scope`'s region. Dispatch tests register builtins
/// whose bodies return distinct markers so the test can assert which overload won.
#[cfg(test)]
pub(crate) fn marker<'a>(scope: &Scope<'a>, label: &'static str) -> &'a KObject<'a> {
    scope.brand().alloc_string(label)
}

/// Seal a resolved value into a region-pure `WorkingPart::Spliced` cell — the test-side peer of
/// the scheduler's splice, so a classification test can build the exact carrier a real splice rests
/// on the working expression. It goes through the production resident door on `host`'s own region
/// handle, so the empty reach is what the mint stamps rather than what a call site asserts.
///
/// `host` is the storage the description is minted into, and it must outlive every read of the
/// returned part: a resting cell owns no pin, so `host` plays the role the region a real splice
/// rests into plays in production. Borrowed at `'a`, which is also the residence check — the door
/// takes a value borrowing for at least the handle's own lifetime, so a `Carried` from anywhere
/// shorter-lived than `host` does not compile.
#[cfg(test)]
pub(crate) fn spliced_part<'a>(
    host: &'a Rc<FrameStorage>,
    c: Carried<'a>,
) -> crate::machine::model::WorkingPart<'a> {
    let brand = RegionBrand(RegionHandle::from_owner(&**host));
    crate::machine::model::WorkingPart::Spliced {
        cell: Sealed::seal(
            brand.seal_resident::<crate::machine::model::CarriedFamily>(c),
            brand.handle(),
        ),
    }
}

/// Build a one-argument signature (`<name: kt>`) returning `Any`.
#[cfg(test)]
pub(crate) fn one_slot_sig<'a>(name: &'a str, kt: KType) -> SignatureDraft<'a> {
    SignatureDraft {
        return_type: ReturnType::Resolved(KType::ANY),
        elements: vec![SignatureElement::Argument(Argument {
            name: crate::machine::model::BinderSymbol::classify(name)
                .expect("a test fixture parameter is a value token"),
            ktype: kt,
        })],
    }
}

/// A run frame held for its registries. The registries are owned by the frame, so reading them off
/// the [`TestRun`] borrows it — which every `run` call needs mutably. Holding the frame instead
/// decouples the two, and keeps the registries readable after the runtime drops.
///
/// Derefs to the type registry, the half nearly every assertion wants.
pub struct RegistryHandle(Rc<CallFrame>);

impl RegistryHandle {
    pub fn registries(&self) -> &RunRegistries {
        self.0
            .registries()
            .expect("run frame carries the registries")
    }
}

impl std::ops::Deref for RegistryHandle {
    type Target = TypeRegistry;
    fn deref(&self) -> &TypeRegistry {
        &self.registries().types
    }
}
