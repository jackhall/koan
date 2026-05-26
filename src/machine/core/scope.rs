use std::cell::RefCell;
use std::io::Write;

use crate::machine::model::ast::KExpression;

use crate::machine::core::kfunction::{ArgumentBundle, KFunction, NodeId};
use crate::machine::model::values::KObject;
use super::arena::RuntimeArena;
use super::bindings::{ApplyOutcome, BindingIndex, Bindings};
use super::kerror::{KError, KErrorKind};
use super::lexical_frame::LexicalFrame;
use super::pending::PendingQueue;
use super::scope_id::ScopeId;

/// Visibility predicate: is `b` visible from a reference at `chain`?
///
/// - `chain = None` (test fixtures, builtin registration paths) ⇒ see everything;
///   the gate is disabled.
/// - `chain.index_for(scope_id) = None` ⇒ the scope isn't on the consumer's chain
///   (the scope is "complete" — a returned-block local or a sibling block fully done).
///   All entries in that scope are visible.
/// - `chain.index_for(scope_id) = Some(c)` ⇒ the cutoff is `c` (this consumer's
///   statement position in the scope). An entry at index `i` is visible iff `i < c`
///   (strict lexical predecessor) OR `b.nominal_binder` is set (D7 carve-out for
///   STRUCT / named UNION / SIG / FUNCTOR / MODULE).
///
/// Builtins (`BindingIndex::BUILTIN`) sit at index 0 and are visible to every user
/// statement: top-level user statement at index 1 has cutoff `1`, and `0 < 1`.
pub(crate) fn visible(scope_id: ScopeId, b: BindingIndex, chain: Option<&LexicalFrame>) -> bool {
    let Some(chain) = chain else {
        return true;
    };
    match chain.index_for(scope_id) {
        None => true,
        Some(c) => b.nominal_binder || b.idx < c,
    }
}

/// A resolved-but-not-yet-executed call: the original expression, the chosen `KFunction`,
/// and the `ArgumentBundle` from `KFunction::bind`. Unit of deferred work in dispatch.
pub struct KFuture<'a> {
    pub parsed: KExpression<'a>,
    pub function: &'a KFunction<'a>,
    pub bundle: ArgumentBundle<'a>,
}

impl<'a> KFuture<'a> {
    /// `function` is shared (arena-allocated, immutable); `parsed` and `bundle` clone deeply.
    pub fn deep_clone(&self) -> KFuture<'a> {
        KFuture {
            parsed: self.parsed.clone(),
            function: self.function,
            bundle: self.bundle.deep_clone(),
        }
    }
}

/// Result of `Scope::resolve`. `Placeholder` carries the producer `NodeId` the consumer
/// should park on (a binder dispatched the name but its body hasn't finalized).
///
/// Invariant: `data` and `placeholders` never both hold the same name in one scope —
/// `bind_value` removes the placeholder before inserting into `data`. Resolution stops at
/// the first scope on the chain that has either.
///
/// Index-gated resolution splits the not-found cases:
/// - [`Resolution::UnboundName`] — no binding visible at this consumer's chain position.
///   Structural absence: either the name is misspelled, or the binding lives at a later
///   sibling that hasn't been ordered yet (the visibility gate hides it).
/// - [`Resolution::Placeholder`] — a binder placeholder is visible and not yet finalized;
///   the consumer parks on the producer `NodeId`.
/// - [`Resolution::Value`] — the binding is finalized and visible.
pub enum Resolution<'a> {
    Value(&'a KObject<'a>),
    Placeholder(NodeId),
    UnboundName,
}

/// Lexical environment. `functions` (inside [`Bindings`]) buckets overloads by their
/// *untyped signature* (token shape with slot types erased) so dispatch can pick between
/// same-shape overloads by `KType` specificity. Only the root scope holds a writer in
/// `out`; child scopes have `None` and `write_out` walks `outer` to find one.
///
/// All mutable binding state lives in the embedded [`Bindings`] façade (interior-mutable
/// `RefCell`s), so a `&'a Scope<'a>` can be shared across scheduler nodes while builtins
/// still mutate through it. Deferred writes that hit a borrow conflict route through the
/// embedded [`PendingQueue`]; `drain_pending` replays them between dispatch nodes.
pub struct Scope<'a> {
    pub outer: Option<&'a Scope<'a>>,
    bindings: ScopeBindings<'a>,
    pub out: RefCell<Option<Box<dyn Write + 'a>>>,
    pub arena: &'a RuntimeArena,
    /// Position-independent identity captured into `KType::UserType { scope_id, .. }` /
    /// `KType::SatisfiesSignature { sig_id, .. }` so dispatch on user-declared types compares
    /// ids rather than scope pointers.
    pub id: ScopeId,
    pending: PendingQueue<'a>,
    pub kind: ScopeKind,
    /// Monotonic counter handing out the next [`LexicalFrame::index`] to a statement
    /// submitted into this scope via [`crate::machine::execute::SchedulerHandle::enter_block`].
    /// Starts at `1` (reserving `0` for the builtin tag — see [`BindingIndex::BUILTIN`])
    /// and advances by `n` per `enter_block` call so a REPL-style series of submissions
    /// against the same scope assigns each statement a unique, monotonically-increasing
    /// position. Re-entry (test fixtures calling `enter_block(scope.id, ...)` repeatedly
    /// against the same scope) continues the count rather than resetting it; the previous
    /// invocations' bindings remain visible because their indices sit strictly less than
    /// the new statements' cutoffs.
    next_statement_index: RefCell<usize>,
}

/// A scope's binding storage. `Owned` is the default — the scope holds its own
/// [`Bindings`] façade. `Borrowed` is the `USING … SCOPE` transparent window: a
/// read-only view onto another scope's façade (the opened module's child-scope
/// bindings). Reads through [`Scope::bindings`] are identical for both arms, so the
/// resolver walk is unchanged; the difference is on the *write* side — a `Borrowed`
/// window has no storage of its own, so [`Scope::bind_value`] /
/// [`Scope::register_function`] / [`Scope::register_type`] forward to `outer` (the
/// call site), which is why block-local binds persist in the enclosing scope after the
/// block ends.
// `Owned` (the common case — every non-`USING` scope) is large and `Borrowed` is a thin
// pointer, but boxing `Owned` would add a heap allocation and an indirection to the hot
// `bindings()` read path that every dispatch walks; `Scope` embedded `Bindings` by value
// regardless, so inlining the large variant is the deliberate trade.
#[allow(clippy::large_enum_variant)]
enum ScopeBindings<'a> {
    Owned(Bindings<'a>),
    /// `&'a Bindings<'a>` (not a shorter borrow) keeps `Scope<'a>` invariant in `'a`,
    /// matching the existing `KFunction`/`Module` lifetime-erasure story. The borrowed
    /// façade lives in the opened module's child-scope arena; the `USING` builtin keeps
    /// that arena alive past the block by rooting the module value (and its frame `Rc`) in
    /// the call-site arena (see [`crate::builtins`]'s `using_scope`).
    Borrowed(&'a Bindings<'a>),
}

impl<'a> ScopeBindings<'a> {
    /// The underlying façade for both arms — the single read path.
    fn get(&self) -> &Bindings<'a> {
        match self {
            ScopeBindings::Owned(b) => b,
            ScopeBindings::Borrowed(b) => b,
        }
    }

    fn is_borrowed(&self) -> bool {
        matches!(self, ScopeBindings::Borrowed(_))
    }
}

/// Lexical classification for a [`Scope`]. The SIG-body gate in `val_decl` and
/// `let_binding` walks outward and pivots on the first non-`Anonymous` variant: `Sig`
/// admits VAL declarators and rejects LET-by-example; `Module` is the opposite. The
/// per-variant `name` field carries the surface label for diagnostics.
#[derive(Debug, Clone)]
pub enum ScopeKind {
    Anonymous,
    Sig { name: String },
    Module { name: String },
}

impl<'a> Scope<'a> {
    pub fn run_root(arena: &'a RuntimeArena, outer: Option<&'a Scope<'a>>, out: Box<dyn Write + 'a>) -> Self {
        Self {
            outer,
            bindings: ScopeBindings::Owned(Bindings::new()),
            out: RefCell::new(Some(out)),
            arena,
            id: ScopeId::next(),
            pending: PendingQueue::new(),
            kind: ScopeKind::Anonymous,
            next_statement_index: RefCell::new(1),
        }
    }

    pub fn child_for_call(&'a self) -> Scope<'a> {
        Self::child_under(self)
    }

    /// `outer` is the lexical parent — for FN bodies this is the captured definition scope,
    /// not the call site.
    pub fn child_under(outer: &'a Scope<'a>) -> Scope<'a> {
        Scope {
            outer: Some(outer),
            bindings: ScopeBindings::Owned(Bindings::new()),
            out: RefCell::new(None),
            arena: outer.arena,
            id: ScopeId::next(),
            pending: PendingQueue::new(),
            kind: ScopeKind::Anonymous,
            next_statement_index: RefCell::new(1),
        }
    }

    /// Like `child_under` but stamps the scope as a SIG decl_scope.
    pub fn child_under_sig(outer: &'a Scope<'a>, name: String) -> Scope<'a> {
        Scope {
            outer: Some(outer),
            bindings: ScopeBindings::Owned(Bindings::new()),
            out: RefCell::new(None),
            arena: outer.arena,
            id: ScopeId::next(),
            pending: PendingQueue::new(),
            kind: ScopeKind::Sig { name },
            next_statement_index: RefCell::new(1),
        }
    }

    /// Like `child_under` but stamps the scope as a MODULE body (also used for the
    /// per-ascription view minted by `:|`).
    pub fn child_under_module(outer: &'a Scope<'a>, name: String) -> Scope<'a> {
        Scope {
            outer: Some(outer),
            bindings: ScopeBindings::Owned(Bindings::new()),
            out: RefCell::new(None),
            arena: outer.arena,
            id: ScopeId::next(),
            pending: PendingQueue::new(),
            kind: ScopeKind::Module { name },
            next_statement_index: RefCell::new(1),
        }
    }

    /// Build a transparent `USING … SCOPE` child scope: `outer` is the call site (the
    /// lexical parent, *not* the opened module's def site), and the bindings are a
    /// read-only [`ScopeBindings::Borrowed`] window onto `module_bindings` (the opened
    /// module's child-scope façade). Reads consult the window first, then walk `outer`;
    /// writes forward to `outer`. `arena` is `outer.arena` (the call-site arena), so the
    /// `USING` builtin can allocate this scope — and every block-body allocation made
    /// through it — in the call site's arena, which is what makes forwarded binds sound
    /// (they outlive the block).
    pub fn child_transparent(outer: &'a Scope<'a>, module_bindings: &'a Bindings<'a>) -> Scope<'a> {
        Scope {
            outer: Some(outer),
            bindings: ScopeBindings::Borrowed(module_bindings),
            out: RefCell::new(None),
            arena: outer.arena,
            id: ScopeId::next(),
            pending: PendingQueue::new(),
            kind: ScopeKind::Anonymous,
            next_statement_index: RefCell::new(1),
        }
    }

    /// Consume the next `count` statement indices from this scope's counter. Used by
    /// [`crate::machine::execute::SchedulerHandle::enter_block`] to assign each statement
    /// a monotonically-increasing position in this scope's source order. Returns the
    /// starting index; the caller hands out `start..(start + count)` to its statements.
    ///
    /// Counters advance across repeated `enter_block` calls against the same scope, so
    /// a test fixture that calls `run(...)` to register bindings and then `run_one(...)`
    /// to read them sees the new call assigned an index strictly greater than the
    /// previous binds. That's what keeps previously-bound names visible (their indices
    /// sit strictly less than the new call's cutoff under the `b.idx < c` predicate).
    pub fn consume_statement_indices(&self, count: usize) -> usize {
        let mut next = self.next_statement_index.borrow_mut();
        let start = *next;
        *next += count;
        start
    }

    pub fn bindings(&self) -> &Bindings<'a> {
        self.bindings.get()
    }

    /// Scope-bound `TypeExpr → &KType` memo read. A transparent `USING` window skips the
    /// cache entirely (returns `None`): its resolutions are influenced by the call-site
    /// chain, so caching them into the *module's* shared memo would poison it for the
    /// module's own def-site resolution.
    pub(crate) fn type_expr_memo_get(
        &self,
        te: &crate::machine::model::ast::TypeExpr,
    ) -> Option<&'a crate::machine::model::types::KType<'a>> {
        if self.bindings.is_borrowed() {
            return None;
        }
        self.bindings.get().type_expr_memo_get(te)
    }

    /// Scope-bound memo write — no-op on a transparent `USING` window (see
    /// [`Self::type_expr_memo_get`]).
    pub(crate) fn type_expr_memo_insert(
        &self,
        te: crate::machine::model::ast::TypeExpr,
        kt: &'a crate::machine::model::types::KType<'a>,
    ) {
        if self.bindings.is_borrowed() {
            return;
        }
        self.bindings.get().type_expr_memo_insert(te, kt);
    }

    /// The call-site scope a `Borrowed` (transparent `USING`) window forwards its writes
    /// to. Panics if `self` is `Borrowed` but rootless — the transparent constructor
    /// always sets `outer` to the call site, so a `Borrowed` scope without an `outer` is
    /// a construction bug.
    fn write_target(&self) -> &Scope<'a> {
        self.outer.expect(
            "a Borrowed (USING transparent) scope must have an outer call-site to forward \
             writes to",
        )
    }

    /// Iterate `self` and its `outer` chain. Per-step `RefCell` guards taken inside a
    /// `find_map` / `find` closure drop at the closure boundary, so a deep chain never
    /// accumulates live read borrows.
    pub fn ancestors(&self) -> impl Iterator<Item = &Scope<'a>> {
        std::iter::once(self).chain(std::iter::successors(self.outer, |s| s.outer))
    }

    /// True iff `self`'s nearest non-`Anonymous` enclosing scope is a SIG decl_scope.
    /// A non-SIG named scope (`Module`) short-circuits to `false`; `Anonymous` frames
    /// are transparent and the walk continues outward.
    pub fn is_in_sig_body(&self) -> bool {
        self.ancestors()
            .find_map(|s| match &s.kind {
                ScopeKind::Sig { .. } => Some(true),
                ScopeKind::Module { .. } => Some(false),
                ScopeKind::Anonymous => None,
            })
            .unwrap_or(false)
    }

    /// Bind `name` in this scope. Errors `Rebind` if `data` already holds `name`
    /// (same-scope rebind rejected; cross-scope shadowing allowed). Removes any matching
    /// placeholder this scope owns on success.
    ///
    /// Conditional-defer: direct mutation first, falls back to the `pending` queue iff a
    /// borrow conflict would otherwise panic (caller up the stack iterating `data`).
    pub fn bind_value(
        &self,
        name: String,
        obj: &'a KObject<'a>,
        index: BindingIndex,
    ) -> Result<(), KError> {
        if self.bindings.is_borrowed() {
            // Transparent `USING` window: reads consult the window before the call site,
            // so a local bind whose name is already a surfaced module member would be
            // silently shadowed by the window. Reject it (the block is unconditional, so
            // there is no divergent-binding hazard the way TRY/MATCH branches have — the
            // only failure mode is this shadowing one). The bind otherwise belongs to the
            // call site, not the read-only module view, so forward it there.
            //
            // The forwarded write carries the call-site `index` unchanged: the bind
            // belongs in the call site's lexical block, at the call site's statement
            // position, not the module's. See D2 in plan-index-gated-resolution.md.
            if self.bindings.get().data().contains_key(&name) {
                return Err(KError::new(KErrorKind::ShapeError(format!(
                    "USING: local bind `{name}` collides with a surfaced module member; \
                     rename it to avoid silently shadowing the module's `{name}`",
                ))));
            }
            return self.write_target().bind_value(name, obj, index);
        }
        match self.bindings.get().try_bind_value(&name, obj, index)? {
            ApplyOutcome::Applied => Ok(()),
            ApplyOutcome::Conflict => {
                self.pending.defer_value(name, obj, index);
                Ok(())
            }
        }
    }

    /// Add `fn_ref` to the `functions` bucket keyed by its untyped signature, then insert
    /// `obj` into `data[name]`. Errors:
    /// - `DuplicateOverload` if the bucket already holds an exact-signature equal function.
    /// - `Rebind` if `data[name]` holds a non-function.
    ///
    /// Same conditional-defer shape as `bind_value`.
    pub fn register_function(
        &self,
        name: String,
        fn_ref: &'a KFunction<'a>,
        obj: &'a KObject<'a>,
        index: BindingIndex,
    ) -> Result<(), KError> {
        if self.bindings.is_borrowed() {
            return self.write_target().register_function(name, fn_ref, obj, index);
        }
        match self.bindings.get().try_register_function(&name, fn_ref, obj, index)? {
            ApplyOutcome::Applied => Ok(()),
            ApplyOutcome::Conflict => {
                self.pending.defer_function(name, fn_ref, obj, index);
                Ok(())
            }
        }
    }

    /// Register `name` as a type-valued binding in this scope. The binding lives in
    /// [`Bindings::types`] as an arena-allocated `&KType`; type-name reads go through
    /// [`Self::resolve_type`]. Same conditional-defer shape as [`Self::bind_value`].
    /// Infallible: a name collision at builtin registration is a programming error,
    /// so the [`KError`] from `try_register_type` is dropped.
    pub fn register_type(
        &self,
        name: String,
        ktype: crate::machine::model::types::KType<'a>,
        index: BindingIndex,
    ) {
        if self.bindings.is_borrowed() {
            self.write_target().register_type(name, ktype, index);
            return;
        }
        let kt_ref: &'a crate::machine::model::types::KType<'a> = self.arena.alloc(ktype);
        match self.bindings.get().try_register_type(&name, kt_ref, index) {
            Ok(ApplyOutcome::Applied) => {}
            Ok(ApplyOutcome::Conflict) => self.pending.defer_type(name, kt_ref, index),
            Err(_) => {}
        }
    }

    /// Synchronous identity install for the SCC cycle-close sweep. Writes `name` →
    /// `ktype` to [`Bindings::types`] via the same primitive [`Self::register_type`]
    /// uses, but panics on borrow conflict instead of deferring through the pending
    /// queue. Panics on `Rebind` too — a cycle member's identity must not already be
    /// in `types` when cycle-close fires.
    ///
    /// Called by [`crate::machine::model::types::resolver::close_type_cycle`] from
    /// inside the elaborator's `Resolution::Placeholder` arm. At that call site no
    /// outer `bindings` borrow is held (the placeholder lookup released its `Ref`
    /// before returning), so a conflict here is a programming error. The downstream
    /// finalize's [`crate::machine::core::Bindings::try_register_nominal`] idempotent
    /// arm picks up the carrier write against this pre-installed identity.
    pub fn cycle_close_install_identity(
        &self,
        name: String,
        ktype: crate::machine::model::types::KType<'a>,
        index: BindingIndex,
    ) {
        if self.bindings.is_borrowed() {
            self.write_target().cycle_close_install_identity(name, ktype, index);
            return;
        }
        let kt_ref: &'a crate::machine::model::types::KType<'a> = self.arena.alloc(ktype);
        match self.bindings.get().try_register_type(&name, kt_ref, index) {
            Ok(ApplyOutcome::Applied) => {}
            Ok(ApplyOutcome::Conflict) => panic!(
                "cycle_close_install_identity borrow conflict on `{name}` — cycle-close \
                 runs from the elaborator with no outer types borrow held",
            ),
            Err(e) => panic!(
                "cycle_close_install_identity Rebind for `{name}`: {e} — cycle member \
                 identity should not already be in bindings.types",
            ),
        }
    }

    /// Transactional dual-write for nominal declarations (STRUCT, named UNION, MODULE,
    /// SIG). Identity `kt` (a `KType::UserType` or `KType::SatisfiesSignature`) is inserted
    /// into [`Bindings::types`] and the runtime carrier `obj` (`StructType`,
    /// `TaggedUnionType`, `KModule`, `KSignature`) into [`Bindings::data`] atomically
    /// via [`Bindings::try_register_nominal`]. Returns the carrier on success so the
    /// caller can yield it back to the dispatcher via `BodyResult::Value`.
    ///
    /// Finalize sites are post-Combine, past the re-entrant queue point: a borrow
    /// `Conflict` here is a programming error. Mirrors [`Self::bind_value`]'s shape:
    /// panic on `Conflict`, return `Err` on `Rebind`.
    pub fn register_nominal(
        &self,
        name: String,
        kt: crate::machine::model::types::KType<'a>,
        obj: &'a KObject<'a>,
        index: BindingIndex,
    ) -> Result<&'a KObject<'a>, KError> {
        if self.bindings.is_borrowed() {
            return self.write_target().register_nominal(name, kt, obj, index);
        }
        let kt_ref: &'a crate::machine::model::types::KType<'a> = self.arena.alloc(kt);
        match self.bindings.get().try_register_nominal(&name, kt_ref, obj, index)? {
            ApplyOutcome::Applied => Ok(obj),
            ApplyOutcome::Conflict => {
                panic!(
                    "register_nominal borrow conflict on `{name}` — finalize sites run \
                     post-Combine outside the re-entrant bind hot path, so a conflict \
                     here indicates a programming error",
                );
            }
        }
    }

    /// Apply queued writes between dispatch nodes. Thin delegation to
    /// [`PendingQueue::drain`] — items that still hit a borrow conflict stay queued
    /// (eventually-consistent, not guaranteed-empty after one call), and drain-time
    /// `Err`s are debug-asserted (production drops them silently, since dispatch nodes
    /// have no caller frame to surface them to).
    pub fn drain_pending(&self) {
        // A transparent `USING` window never queues into its own `pending` (writes
        // forward to the call site, which queues into *its* pending), so flush the call
        // site instead. Forwarding keeps the call site's deferred binds eventually
        // applied when the block's node finishes a step.
        if self.bindings.is_borrowed() {
            self.write_target().drain_pending();
            return;
        }
        self.pending.drain(self.bindings.get());
    }

    /// Walk the `outer` chain for the nearest value binding of `name`. Wrapper over
    /// [`Scope::resolve`] that collapses `Placeholder` and `UnboundName` to `None`.
    /// Visibility is unfiltered (test fixtures / builtin paths); use
    /// [`Self::lookup_with_chain`] from a dispatch-driven path.
    pub fn lookup(&self, name: &str) -> Option<&'a KObject<'a>> {
        self.lookup_with_chain(name, None)
    }

    /// Chain-gated companion to [`Self::lookup`]. Same outcome contract; the visibility
    /// filter consults `chain` per the predicate in [`visible`].
    pub fn lookup_with_chain(
        &self,
        name: &str,
        chain: Option<&LexicalFrame>,
    ) -> Option<&'a KObject<'a>> {
        match self.resolve_with_chain(name, chain) {
            Resolution::Value(v) => Some(v),
            Resolution::Placeholder(_) | Resolution::UnboundName => None,
        }
    }

    /// Resolve `name` against this scope and the `outer` chain. **Stops at the first hit
    /// per scope, checking `data` then `placeholders`** — an inner scope's placeholder
    /// shadows an outer scope's value binding for the same name (the inner producer hasn't
    /// finalized yet, so the consumer must park on it rather than read through to the outer).
    ///
    /// Type-side bindings (`bindings.types`) are *not* consulted here — type-name reads
    /// go through [`Self::resolve_type`].
    ///
    /// Visibility is unfiltered (test fixtures bypass the scheduler). For dispatch-driven
    /// reads use [`Self::resolve_with_chain`].
    pub fn resolve(&self, name: &str) -> Resolution<'a> {
        self.resolve_with_chain(name, None)
    }

    /// Chain-gated companion to [`Self::resolve`]. Per-scope `data` and `placeholders`
    /// hits are filtered through the visibility predicate (see [`visible`]) before
    /// being returned. Hidden entries (later siblings, or value-style binders before
    /// their lexical position) are skipped, so the walk continues to the next ancestor
    /// scope — matching the index-gated resolution rule:
    ///
    /// > a binding is visible iff, walking the consumer's lexical scope chain to the
    /// > binding's block, the binding's index is strictly less than the consumer's index
    /// > (or the binding's nominal-binder flag is set, the D7 carve-out).
    pub fn resolve_with_chain(
        &self,
        name: &str,
        chain: Option<&LexicalFrame>,
    ) -> Resolution<'a> {
        self.ancestors()
            .find_map(|scope| {
                if let Some((obj, idx)) = scope.bindings().data().get(name).copied() {
                    if visible(scope.id, idx, chain) {
                        return Some(Resolution::Value(obj));
                    }
                }
                if let Some((id, idx)) =
                    scope.bindings().placeholders().get(name).copied()
                {
                    if visible(scope.id, idx, chain) {
                        return Some(Resolution::Placeholder(id));
                    }
                }
                None
            })
            .unwrap_or(Resolution::UnboundName)
    }

    /// Install a dispatch-time placeholder for `name` -> producer slot `idx`. Thin shim
    /// over [`Bindings::try_install_placeholder`] — see that method's docstring for the
    /// `Rebind` rules and the asymmetry with `try_bind_*` (panics on borrow conflict
    /// rather than queueing).
    pub fn install_placeholder(
        &self,
        name: String,
        idx: NodeId,
        index: BindingIndex,
    ) -> Result<(), KError> {
        if self.bindings.is_borrowed() {
            return self.write_target().install_placeholder(name, idx, index);
        }
        self.bindings.get().try_install_placeholder(name, idx, index)
    }

    /// Bucket-keyed companion to [`Self::install_placeholder`] — installs a
    /// `pending_overloads[bucket] = NodeId(idx)` entry so `resolve_dispatch`'s
    /// no-bucket fallback parks bare-arg calls on the producing FN/FUNCTOR
    /// binder. Forwards through the `Borrowed` window the same way as the
    /// name-based companion. See [`Bindings::try_install_pending_overload`].
    pub fn install_pending_overload(
        &self,
        bucket: crate::machine::model::types::UntypedKey,
        idx: NodeId,
        index: BindingIndex,
    ) -> Result<(), KError> {
        if self.bindings.is_borrowed() {
            return self.write_target().install_pending_overload(bucket, idx, index);
        }
        self.bindings.get().try_install_pending_overload(bucket, idx, index)
    }

    /// Walk the `outer` chain for the nearest `bindings.types[name]`. Type-side
    /// analogue of [`Self::lookup`] — no `Placeholder` variant. Visibility unfiltered;
    /// dispatch-driven reads use [`Self::resolve_type_with_chain`].
    pub fn resolve_type(&self, name: &str) -> Option<&'a crate::machine::model::types::KType<'a>> {
        self.resolve_type_with_chain(name, None)
    }

    /// Chain-gated companion to [`Self::resolve_type`]. Per-scope `types` hits are
    /// filtered through the same [`visible`] predicate the value-side resolver uses,
    /// so a type binding declared lexically later in the same block is invisible to
    /// an earlier sibling (unless the binder is a nominal-binder carve-out — STRUCT,
    /// SIG, FUNCTOR, MODULE, named UNION).
    pub fn resolve_type_with_chain(
        &self,
        name: &str,
        chain: Option<&LexicalFrame>,
    ) -> Option<&'a crate::machine::model::types::KType<'a>> {
        self.ancestors().find_map(|scope| {
            scope.bindings().types().get(name).and_then(|(kt, idx)| {
                if visible(scope.id, *idx, chain) {
                    Some(*kt)
                } else {
                    None
                }
            })
        })
    }

    /// Walk the `outer` chain for the nearest scope holding a writer and write `bytes`.
    /// Writer errors are silently dropped.
    pub fn write_out(&self, bytes: &[u8]) {
        for scope in self.ancestors() {
            if let Some(w) = scope.out.borrow_mut().as_mut() {
                let _ = w.write_all(bytes);
                return;
            }
        }
    }

    pub fn lookup_kfunction(&self, name: &str) -> Option<&'a KFunction<'a>> {
        match self.lookup(name)? {
            KObject::KFunction(f, _) => Some(*f),
            _ => None,
        }
    }

}

