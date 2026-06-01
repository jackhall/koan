use std::cell::RefCell;
use std::io::Write;

use crate::machine::model::ast::KExpression;

use super::arena::RuntimeArena;
pub use super::bindings::Resolution;
use super::bindings::{ApplyOutcome, BindingIndex, Bindings};
use super::kerror::{KError, KErrorKind};
use super::lexical_frame::LexicalFrame;
use super::pending::PendingQueue;
use super::scope_id::ScopeId;
use crate::machine::core::kfunction::{ArgumentBundle, KFunction, NodeId};
use crate::machine::model::values::KObject;

/// Index-gated visibility predicate. Production lookups apply this inside
/// [`Bindings::lookup_value`] / [`Bindings::lookup_type`] /
/// [`Bindings::lookup_function`] after translating `Option<&LexicalFrame>`
/// into a per-scope cutoff via [`LexicalFrame::index_for`]. Kept as the
/// predicate's documented home.
///
/// - `chain = None` (test fixtures, builtin registration) — gate disabled.
/// - `chain.index_for(scope_id) = None` — scope is off the consumer's chain
///   (a completed sibling block); everything visible.
/// - `chain.index_for(scope_id) = Some(c)` — visible iff `b.idx < c` OR
///   `b.nominal_binder` (D7 carve-out for STRUCT / named UNION / SIG /
///   FUNCTOR / MODULE).
#[allow(dead_code)]
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

/// Lexical environment. Only the root scope holds a writer in `out`; child scopes
/// have `None` and `write_out` walks `outer` to find one.
///
/// All mutable binding state lives in the embedded [`Bindings`] façade
/// (interior-mutable `RefCell`s), so a `&'a Scope<'a>` is shareable across scheduler
/// nodes. Writes that hit a borrow conflict route through [`PendingQueue`];
/// `drain_pending` replays them between dispatch nodes.
pub struct Scope<'a> {
    pub outer: Option<&'a Scope<'a>>,
    bindings: ScopeBindings<'a>,
    pub out: RefCell<Option<Box<dyn Write + 'a>>>,
    pub arena: &'a RuntimeArena,
    /// Position-independent identity captured into `KType::UserType { scope_id, .. }` /
    /// `KType::Signature { sig, .. }` (via `sig.sig_id()`) so dispatch on user-declared
    /// types compares ids rather than scope pointers.
    pub id: ScopeId,
    pending: PendingQueue<'a>,
    pub kind: ScopeKind,
}

/// A scope's binding storage. `Owned` is the default. `Borrowed` is the
/// `USING … SCOPE` transparent window: a read-only view onto another scope's
/// façade. Writes through a `Borrowed` window forward to `outer` (the call site),
/// so block-local binds persist after the block ends.
// Boxing `Owned` would add an allocation and an indirection on the hot `bindings()`
// read path; inlining the large variant is the deliberate trade.
#[allow(clippy::large_enum_variant)]
enum ScopeBindings<'a> {
    Owned(Bindings<'a>),
    /// `&'a Bindings<'a>` (not a shorter borrow) keeps `Scope<'a>` invariant in `'a`.
    /// The borrowed façade lives in the opened module's child-scope arena; the
    /// `USING` builtin keeps that arena alive by rooting the module value in the
    /// call-site arena.
    Borrowed(&'a Bindings<'a>),
}

impl<'a> ScopeBindings<'a> {
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

/// Lexical classification for a [`Scope`]. The SIG-body gate walks outward and
/// pivots on the first non-`Anonymous` variant: `Sig` admits VAL declarators and
/// rejects LET-by-example; `Module` is the opposite. The per-variant `name` field
/// is the surface label for diagnostics.
#[derive(Debug, Clone)]
pub enum ScopeKind {
    Anonymous,
    Sig { name: String },
    Module { name: String },
}

impl<'a> Scope<'a> {
    pub fn run_root(
        arena: &'a RuntimeArena,
        outer: Option<&'a Scope<'a>>,
        out: Box<dyn Write + 'a>,
    ) -> Self {
        Self {
            outer,
            bindings: ScopeBindings::Owned(Bindings::new()),
            out: RefCell::new(Some(out)),
            arena,
            id: ScopeId::next(),
            pending: PendingQueue::new(),
            kind: ScopeKind::Anonymous,
        }
    }

    pub fn child_for_call(&'a self) -> Scope<'a> {
        Self::child_under(self)
    }

    /// `outer` is the lexical parent — for FN bodies the captured definition scope,
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
        }
    }

    /// `child_under`, stamped as a SIG decl_scope.
    pub fn child_under_sig(outer: &'a Scope<'a>, name: String) -> Scope<'a> {
        Scope {
            outer: Some(outer),
            bindings: ScopeBindings::Owned(Bindings::new()),
            out: RefCell::new(None),
            arena: outer.arena,
            id: ScopeId::next(),
            pending: PendingQueue::new(),
            kind: ScopeKind::Sig { name },
        }
    }

    /// `child_under`, stamped as a MODULE body (also used for the per-ascription view
    /// minted by `:|`).
    pub fn child_under_module(outer: &'a Scope<'a>, name: String) -> Scope<'a> {
        Scope {
            outer: Some(outer),
            bindings: ScopeBindings::Owned(Bindings::new()),
            out: RefCell::new(None),
            arena: outer.arena,
            id: ScopeId::next(),
            pending: PendingQueue::new(),
            kind: ScopeKind::Module { name },
        }
    }

    /// Transparent `USING … SCOPE` child scope. `outer` is the call site (the lexical
    /// parent, not the opened module's def site); bindings are a read-only window onto
    /// `module_bindings`. Reads consult the window first then walk `outer`; writes
    /// forward to `outer`. `arena` is `outer.arena` so block-body allocations outlive
    /// the block (forwarded binds are sound).
    pub fn child_transparent(outer: &'a Scope<'a>, module_bindings: &'a Bindings<'a>) -> Scope<'a> {
        Scope {
            outer: Some(outer),
            bindings: ScopeBindings::Borrowed(module_bindings),
            out: RefCell::new(None),
            arena: outer.arena,
            id: ScopeId::next(),
            pending: PendingQueue::new(),
            kind: ScopeKind::Anonymous,
        }
    }

    pub fn bindings(&self) -> &Bindings<'a> {
        self.bindings.get()
    }

    /// Scope-bound `TypeExpr → &KType` memo read. A transparent `USING` window returns
    /// `None`: its resolutions depend on the call-site chain, so caching them into the
    /// module's shared memo would poison the module's own def-site resolution.
    pub(crate) fn type_expr_memo_get(
        &self,
        te: &crate::machine::model::ast::TypeExpr,
    ) -> Option<&'a crate::machine::model::types::KType<'a>> {
        if self.bindings.is_borrowed() {
            return None;
        }
        self.bindings.get().type_expr_memo_get(te)
    }

    /// Memo write — no-op on a transparent `USING` window (see
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

    /// Call-site scope a `Borrowed` window forwards writes to. Panics if `Borrowed`
    /// but rootless — the transparent constructor always sets `outer`, so this would
    /// be a construction bug.
    fn write_target(&self) -> &Scope<'a> {
        self.outer.expect(
            "a Borrowed (USING transparent) scope must have an outer call-site to forward \
             writes to",
        )
    }

    /// Iterate `self` and its `outer` chain. Per-step `RefCell` guards taken inside a
    /// `find_map` / `find` closure drop at the closure boundary, so a deep walk never
    /// accumulates live read borrows.
    pub fn ancestors(&self) -> impl Iterator<Item = &Scope<'a>> {
        std::iter::once(self).chain(std::iter::successors(self.outer, |s| s.outer))
    }

    /// True iff the nearest non-`Anonymous` enclosing scope is a SIG decl_scope. A
    /// `Module` short-circuits to `false`; `Anonymous` frames are transparent.
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
    /// (same-scope rebind rejected; cross-scope shadowing allowed). Removes any
    /// matching placeholder this scope owns on success.
    ///
    /// Conditional-defer: direct mutation first, falls back to the `pending` queue
    /// iff a borrow conflict would otherwise panic.
    pub fn bind_value(
        &self,
        name: String,
        obj: &'a KObject<'a>,
        index: BindingIndex,
    ) -> Result<(), KError> {
        if self.bindings.is_borrowed() {
            // Transparent `USING` window: reads consult the window before the call
            // site, so a local bind whose name is already a surfaced module member
            // would be silently shadowed. Reject it; otherwise forward to the call
            // site under the caller's `index` (the bind belongs to the call site's
            // block, at the call site's statement position).
            if matches!(
                self.bindings.get().lookup_value(&name, None),
                Some(Resolution::Value(_))
            ) {
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

    /// Add `fn_ref` to the `functions` bucket keyed by its untyped signature, then
    /// insert `obj` into `data[name]`. Errors:
    /// - `DuplicateOverload` if the bucket already holds an exact-signature match.
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
            return self
                .write_target()
                .register_function(name, fn_ref, obj, index);
        }
        match self
            .bindings
            .get()
            .try_register_function(&name, fn_ref, obj, index)?
        {
            ApplyOutcome::Applied => Ok(()),
            ApplyOutcome::Conflict => {
                self.pending.defer_function(name, fn_ref, obj, index);
                Ok(())
            }
        }
    }

    /// Register `name` as a type-valued binding. Lives in [`Bindings::types`] as an
    /// arena-allocated `&KType`; reads go through [`Self::resolve_type`]. Same
    /// conditional-defer shape as [`Self::bind_value`]. Infallible: a name collision
    /// at builtin registration is a programming error, so the [`KError`] is dropped.
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

    /// Upsert install for a type-only nominal finalize (STRUCT / named UNION / Result /
    /// MODULE). Writes the schema-bearing identity into [`Bindings::types`], overwriting
    /// a `PartialEq`-equal payload-empty identity the SCC cycle-close pre-installed.
    /// Returns the arena-allocated `&KType` so the caller can yield it as a
    /// `KObject::KTypeValue`. Same conditional-defer shape as [`Self::register_type`];
    /// `Err(Rebind)` on a genuine non-equal collision.
    ///
    /// Finalize runs post-Combine, past the re-entrant queue point — a `Conflict` here
    /// is a programming error, so it panics rather than deferring (deferring would risk
    /// a window where the type resolves with the pre-install's empty payload).
    pub fn register_type_upsert(
        &self,
        name: String,
        ktype: crate::machine::model::types::KType<'a>,
        index: BindingIndex,
    ) -> Result<&'a crate::machine::model::types::KType<'a>, KError> {
        if self.bindings.is_borrowed() {
            return self.write_target().register_type_upsert(name, ktype, index);
        }
        let kt_ref: &'a crate::machine::model::types::KType<'a> = self.arena.alloc(ktype);
        match self
            .bindings
            .get()
            .try_register_type_upsert(&name, kt_ref, index)?
        {
            ApplyOutcome::Applied => Ok(kt_ref),
            ApplyOutcome::Conflict => panic!(
                "register_type_upsert borrow conflict on `{name}` — nominal finalize sites \
                 run post-Combine outside the re-entrant bind hot path",
            ),
        }
    }

    /// Synchronous identity install for the SCC cycle-close sweep. Writes `name` →
    /// `ktype` to [`Bindings::types`], but panics on borrow conflict instead of
    /// deferring, and panics on `Rebind` — a cycle member's identity must not already
    /// be in `types` when cycle-close fires.
    ///
    /// Cycle-close runs from the elaborator's `Resolution::Placeholder` arm with no
    /// outer `bindings` borrow held; a conflict here is a programming error. The
    /// identity installed here is payload-empty (schema not yet elaborated); the
    /// downstream finalize overwrites it with the schema-bearing one via
    /// [`Self::register_type_upsert`].
    pub fn cycle_close_install_identity(
        &self,
        name: String,
        ktype: crate::machine::model::types::KType<'a>,
        index: BindingIndex,
    ) {
        if self.bindings.is_borrowed() {
            self.write_target()
                .cycle_close_install_identity(name, ktype, index);
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

    /// Apply queued writes between dispatch nodes. Items that still hit a borrow
    /// conflict stay queued (eventually-consistent), and drain-time `Err`s are
    /// debug-asserted (production drops them — dispatch nodes have no caller frame to
    /// surface them to).
    pub fn drain_pending(&self) {
        // Transparent `USING` window writes forward to the call site, so its pending
        // queue lives there too — flush the call site.
        if self.bindings.is_borrowed() {
            self.write_target().drain_pending();
            return;
        }
        self.pending.drain(self.bindings.get());
    }

    /// Nearest value binding of `name` up the `outer` chain. Collapses `Placeholder`
    /// and `UnboundName` to `None`. Visibility unfiltered — use
    /// [`Self::lookup_with_chain`] from a dispatch-driven path.
    pub fn lookup(&self, name: &str) -> Option<&'a KObject<'a>> {
        self.lookup_with_chain(name, None)
    }

    /// Chain-gated companion to [`Self::lookup`]. Filter consults `chain` per
    /// [`visible`].
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

    /// Resolve `name` against this scope and the `outer` chain. Stops at the first
    /// per-scope hit, checking `data` then `placeholders` — an inner placeholder
    /// shadows an outer value binding, because the inner producer hasn't finalized
    /// and the consumer must park rather than read through.
    ///
    /// Type-side bindings are not consulted — see [`Self::resolve_type`].
    /// Visibility unfiltered; dispatch-driven reads use [`Self::resolve_with_chain`].
    pub fn resolve(&self, name: &str) -> Resolution<'a> {
        self.resolve_with_chain(name, None)
    }

    /// Chain-gated companion to [`Self::resolve`]. Per-scope hits are filtered through
    /// [`visible`] before being returned; hidden entries (later siblings, or
    /// value-style binders before their lexical position) are skipped and the walk
    /// continues outward.
    pub fn resolve_with_chain(&self, name: &str, chain: Option<&LexicalFrame>) -> Resolution<'a> {
        self.ancestors()
            .find_map(|scope| {
                let cutoff = chain.and_then(|c| c.index_for(scope.id));
                scope.bindings().lookup_value(name, cutoff)
            })
            .unwrap_or(Resolution::UnboundName)
    }

    /// Install a dispatch-time placeholder for `name` -> producer slot `idx`. See
    /// [`Bindings::try_install_placeholder`] for `Rebind` rules and the asymmetry with
    /// `try_bind_*` (panics on borrow conflict rather than queueing).
    pub fn install_placeholder(
        &self,
        name: String,
        idx: NodeId,
        index: BindingIndex,
    ) -> Result<(), KError> {
        if self.bindings.is_borrowed() {
            return self.write_target().install_placeholder(name, idx, index);
        }
        self.bindings
            .get()
            .try_install_placeholder(name, idx, index)
    }

    /// Bucket-keyed companion to [`Self::install_placeholder`]: appends a
    /// `pending_overloads[bucket]` entry so dispatch's no-bucket fallback parks
    /// bare-arg calls on the producing FN/FUNCTOR binder. Sibling installs sharing the
    /// bucket each append a distinct entry; entries are removed on finalize by
    /// matching the producing binder's `BindingIndex`. See
    /// [`Bindings::try_install_pending_overload`].
    pub fn install_pending_overload(
        &self,
        bucket: crate::machine::model::types::UntypedKey,
        idx: NodeId,
        index: BindingIndex,
    ) -> Result<(), KError> {
        if self.bindings.is_borrowed() {
            return self
                .write_target()
                .install_pending_overload(bucket, idx, index);
        }
        self.bindings
            .get()
            .try_install_pending_overload(bucket, idx, index)
    }

    /// Type-side analogue of [`Self::lookup`] — no `Placeholder` variant. Visibility
    /// unfiltered; dispatch-driven reads use [`Self::resolve_type_with_chain`].
    pub fn resolve_type(&self, name: &str) -> Option<&'a crate::machine::model::types::KType<'a>> {
        self.resolve_type_with_chain(name, None)
    }

    /// Chain-gated companion to [`Self::resolve_type`]. Per-scope `types` hits are
    /// filtered through [`visible`], so a type binding declared lexically later in
    /// the same block is invisible to an earlier sibling (unless the binder is a
    /// nominal-binder carve-out).
    pub fn resolve_type_with_chain(
        &self,
        name: &str,
        chain: Option<&LexicalFrame>,
    ) -> Option<&'a crate::machine::model::types::KType<'a>> {
        self.ancestors().find_map(|scope| {
            let cutoff = chain.and_then(|c| c.index_for(scope.id));
            scope.bindings().lookup_type(name, cutoff)
        })
    }

    /// Write `bytes` to the nearest writer up the `outer` chain. Writer errors are
    /// silently dropped.
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
