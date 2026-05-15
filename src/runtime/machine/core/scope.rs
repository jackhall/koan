use std::cell::RefCell;
use std::io::Write;

use crate::ast::KExpression;

use crate::runtime::machine::kfunction::{ArgumentBundle, KFunction, NodeId};
use crate::runtime::machine::model::values::KObject;
use super::arena::RuntimeArena;
use super::bindings::{ApplyOutcome, Bindings};
use super::kerror::KError;
use super::pending::PendingQueue;

/// A resolved-but-not-yet-executed call: the original expression, the chosen `KFunction`,
/// and the `ArgumentBundle` from `KFunction::bind`. Unit of deferred work in dispatch.
pub struct KFuture<'a> {
    pub parsed: KExpression<'a>,
    pub function: &'a KFunction<'a>,
    pub bundle: ArgumentBundle<'a>,
}

impl<'a> KFuture<'a> {
    /// `function` is shared (arena-allocated, immutable); `parsed` and `bundle` clone deeply
    /// so the result is independent of the original.
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
pub enum Resolution<'a> {
    Value(&'a KObject<'a>),
    Placeholder(NodeId),
    Unbound,
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
    /// All three binding maps live here — public so test fixtures can read them as
    /// `scope.bindings.data()` etc., but writes only flow through the methods.
    pub bindings: Bindings<'a>,
    pub out: RefCell<Option<Box<dyn Write + 'a>>>,
    pub arena: &'a RuntimeArena,
    /// Writes that hit a borrow conflict at `bind_value` / `register_function` time.
    /// Drained between dispatch nodes by `drain_pending`; direct writes bypass the queue.
    /// See [`PendingQueue`] for the deferral / retry surface.
    pending: PendingQueue<'a>,
    /// Lexical-context classification set at construction. `Anonymous` for run-root and
    /// ordinary call frames; `Sig` / `Module` for the named decl-scope variants stamped
    /// by `sig_def`, `module_def`, and `ascribe`. Read by `val_decl` / `let_binding`'s
    /// SIG-body gate; the per-variant `name` field is record-only (carries the surface
    /// label for diagnostics).
    pub kind: ScopeKind,
}

/// Lexical classification for a [`Scope`]. The SIG-body gate in `val_decl` and
/// `let_binding` walks outward from the active scope and pivots on the first non-
/// `Anonymous` variant: `Sig` means "value-slot declarators (VAL) are admitted,
/// LET-by-example is rejected"; `Module` means the opposite. Extend with `Function`
/// or other variants when a caller actually stamps them — kept minimal today.
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
            bindings: Bindings::new(),
            out: RefCell::new(Some(out)),
            arena,
            pending: PendingQueue::new(),
            kind: ScopeKind::Anonymous,
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
            bindings: Bindings::new(),
            out: RefCell::new(None),
            arena: outer.arena,
            pending: PendingQueue::new(),
            kind: ScopeKind::Anonymous,
        }
    }

    /// Like `child_under` but stamps the scope as a SIG decl_scope. The SIG-body gate
    /// in `val_decl` / `let_binding` returns true at the first such scope on the outer
    /// walk.
    pub fn child_under_sig(outer: &'a Scope<'a>, name: String) -> Scope<'a> {
        Scope {
            outer: Some(outer),
            bindings: Bindings::new(),
            out: RefCell::new(None),
            arena: outer.arena,
            pending: PendingQueue::new(),
            kind: ScopeKind::Sig { name },
        }
    }

    /// Like `child_under` but stamps the scope as a MODULE body (also used for the
    /// per-ascription view minted by `:|`). The SIG-body gate returns false at the
    /// first such scope on the outer walk.
    pub fn child_under_module(outer: &'a Scope<'a>, name: String) -> Scope<'a> {
        Scope {
            outer: Some(outer),
            bindings: Bindings::new(),
            out: RefCell::new(None),
            arena: outer.arena,
            pending: PendingQueue::new(),
            kind: ScopeKind::Module { name },
        }
    }

    /// Borrow the embedded [`Bindings`] façade. Internal callers that need direct access to
    /// the unified write path (e.g. ascription's `try_bulk_install_from`) reach for this;
    /// the shim methods below cover the common cases.
    pub fn bindings(&self) -> &Bindings<'a> {
        &self.bindings
    }

    /// True iff `self`'s nearest non-`Anonymous` enclosing scope is a SIG decl_scope.
    /// The walk starts at `self` — a SIG-body builtin's body runs against the SIG
    /// decl_scope directly. A non-SIG named scope (`Module`) short-circuits to `false`;
    /// `Anonymous` frames are transparent and the walk continues outward.
    pub fn is_in_sig_body(&self) -> bool {
        let mut current: Option<&Scope<'_>> = Some(self);
        while let Some(s) = current {
            match &s.kind {
                ScopeKind::Sig { .. } => return true,
                ScopeKind::Module { .. } => return false,
                ScopeKind::Anonymous => current = s.outer,
            }
        }
        false
    }

    /// Bind `name` in this scope. Errors `Rebind` if `data` already holds `name`
    /// (same-scope rebind rejected; cross-scope shadowing allowed). Removes any matching
    /// placeholder this scope owns on success.
    ///
    /// Conditional-defer: direct mutation first, falls back to the `pending` queue iff a
    /// borrow conflict would otherwise panic (caller up the stack iterating `data`).
    pub fn bind_value(&self, name: String, obj: &'a KObject<'a>) -> Result<(), KError> {
        match self.bindings.try_bind_value(&name, obj)? {
            ApplyOutcome::Applied => Ok(()),
            ApplyOutcome::Conflict => {
                self.pending.defer_value(name, obj);
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
    ) -> Result<(), KError> {
        match self.bindings.try_register_function(&name, fn_ref, obj)? {
            ApplyOutcome::Applied => Ok(()),
            ApplyOutcome::Conflict => {
                self.pending.defer_function(name, fn_ref, obj);
                Ok(())
            }
        }
    }

    /// Register `name` as a type-valued binding in this scope. The binding lives in
    /// [`Bindings::types`] as an arena-allocated `&KType` — the dedicated type-side
    /// storage introduced in stage 1.2 of per-type identity. No `KObject::KTypeValue`
    /// wrap at the storage layer. Type-name reads go through [`Self::resolve_type`]
    /// (post-stage-1.5), with the sole `KObject::KTypeValue` synthesis site for
    /// dispatch transport living in
    /// [`crate::runtime::builtins::value_lookup::body_type_expr`].
    ///
    /// Same conditional-defer shape as [`Self::bind_value`] and
    /// [`Self::register_function`]: direct write first, queue through
    /// [`PendingQueue::defer_type`] on borrow conflict. Infallible like the prior
    /// implementation — a name collision at builtin registration is a programming
    /// error, so the [`KError`] from `try_register_type` is dropped.
    pub fn register_type(&self, name: String, ktype: crate::runtime::machine::model::types::KType) {
        let kt_ref: &'a crate::runtime::machine::model::types::KType = self.arena.alloc_ktype(ktype);
        match self.bindings.try_register_type(&name, kt_ref) {
            Ok(ApplyOutcome::Applied) => {}
            Ok(ApplyOutcome::Conflict) => self.pending.defer_type(name, kt_ref),
            Err(_) => {} // see docstring: collisions at builtin registration are bugs.
        }
    }

    /// Synchronous identity install for the stage-3.2 SCC cycle-close sweep. Writes
    /// `name` → `ktype` to [`Bindings::types`] via the same primitive
    /// [`Self::register_type`] uses, but panics on borrow conflict instead of
    /// deferring through the pending queue. Panics on `Rebind` too — a cycle
    /// member's identity must not already be in `types` when cycle-close fires.
    ///
    /// Called by [`crate::runtime::machine::model::types::resolver::close_type_cycle`] from
    /// inside the elaborator's `Resolution::Placeholder` arm. At that call site no
    /// outer `bindings` borrow is held (the placeholder lookup released its `Ref`
    /// before returning), so a conflict here is a programming error. The
    /// downstream finalize's
    /// [`crate::runtime::machine::core::Bindings::try_register_nominal`] idempotent
    /// arm picks up the carrier write against this pre-installed identity.
    pub fn cycle_close_install_identity(
        &self,
        name: String,
        ktype: crate::runtime::machine::model::types::KType,
    ) {
        let kt_ref: &'a crate::runtime::machine::model::types::KType = self.arena.alloc_ktype(ktype);
        match self.bindings.try_register_type(&name, kt_ref) {
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
    /// SIG). Identity `kt` (a `KType::UserType` or `KType::SignatureBound`) is inserted
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
        kt: crate::runtime::machine::model::types::KType,
        obj: &'a KObject<'a>,
    ) -> Result<&'a KObject<'a>, KError> {
        let kt_ref: &'a crate::runtime::machine::model::types::KType = self.arena.alloc_ktype(kt);
        match self.bindings.try_register_nominal(&name, kt_ref, obj)? {
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
        self.pending.drain(&self.bindings);
    }

    /// Walk the `outer` chain for the nearest value binding of `name`. Wrapper over
    /// [`Scope::resolve`] that collapses `Placeholder` and `Unbound` to `None`.
    pub fn lookup(&self, name: &str) -> Option<&'a KObject<'a>> {
        match self.resolve(name) {
            Resolution::Value(v) => Some(v),
            Resolution::Placeholder(_) | Resolution::Unbound => None,
        }
    }

    /// Resolve `name` against this scope and the `outer` chain. **Stops at the first hit
    /// per scope, checking `data` then `placeholders`** — an inner scope's placeholder
    /// shadows an outer scope's value binding for the same name (the inner producer hasn't
    /// finalized yet, so the consumer must park on it rather than read through to the outer).
    ///
    /// Type-side bindings (`bindings.types`) are *not* consulted here — type-name reads
    /// go through [`Self::resolve_type`] post-stage-1.5. The brief stage-1.4 fallback
    /// arm that synthesized a `KObject::KTypeValue` on demand is gone.
    pub fn resolve(&self, name: &str) -> Resolution<'a> {
        if let Some(obj) = self.bindings.data().get(name).copied() {
            return Resolution::Value(obj);
        }
        if let Some(id) = self.bindings.placeholders().get(name).copied() {
            return Resolution::Placeholder(id);
        }
        match self.outer {
            Some(outer) => outer.resolve(name),
            None => Resolution::Unbound,
        }
    }

    /// Install a dispatch-time placeholder for `name` -> producer slot `idx`. Thin shim
    /// over [`Bindings::try_install_placeholder`] — see that method's docstring for the
    /// `Rebind` rules and the asymmetry with `try_bind_*` (panics on borrow conflict
    /// rather than queueing).
    pub fn install_placeholder(&self, name: String, idx: NodeId) -> Result<(), KError> {
        self.bindings.try_install_placeholder(name, idx)
    }

    /// Walk the `outer` chain for the nearest `bindings.types[name]`. Type-side
    /// analogue of [`Self::lookup`] — no `Placeholder` variant (that lane is
    /// reserved for stage 3's `pending_types` registry).
    ///
    /// Drops the `Ref<'_, _>` returned by [`Bindings::types`] before recursing into
    /// `outer` so a deep chain doesn't accumulate live read borrows — same NLL-safe
    /// release-before-recurse discipline as [`Self::resolve_dispatch`].
    pub fn resolve_type(&self, name: &str) -> Option<&'a crate::runtime::machine::model::types::KType> {
        let mut current: Option<&Scope<'a>> = Some(self);
        while let Some(scope) = current {
            let types_guard = scope.bindings.types();
            if let Some(kt) = types_guard.get(name).copied() {
                return Some(kt);
            }
            drop(types_guard);
            current = scope.outer;
        }
        None
    }

    /// Walk the `outer` chain for the nearest scope holding a writer and write `bytes`.
    /// Writer errors are silently dropped.
    pub fn write_out(&self, bytes: &[u8]) {
        if let Some(w) = self.out.borrow_mut().as_mut() {
            let _ = w.write_all(bytes);
            return;
        }
        if let Some(outer) = self.outer {
            outer.write_out(bytes);
        }
    }

    pub fn lookup_kfunction(&self, name: &str) -> Option<&'a KFunction<'a>> {
        match self.lookup(name)? {
            KObject::KFunction(f, _) => Some(*f),
            _ => None,
        }
    }

    /// Single-pass overload resolution: walks `outer` performing **strict-then-tentative
    /// per scope** (both passes consult the same scope's bucket before descending), so an
    /// inner-scope tentative match shadows an outer-scope strict one, mirroring lexical
    /// scoping. Ambiguity surfaces at the first scope where the strict pass ties — it does
    /// NOT fall through to `outer` (silently shadowing an inner conflict would hide a real
    /// author error).
    ///
    /// Outcomes:
    /// - [`ResolveOutcome::Resolved`]: a unique overload was picked. The carried
    ///   [`Resolved`] bundles the function plus the per-slot classification
    ///   ([`KFunction::classify_for_pick`]) plus an optional `placeholder_name` extracted
    ///   from the picked function's `pre_run` (the binder-side name to install at dispatch
    ///   time).
    /// - [`ResolveOutcome::Ambiguous(n)`]: the strict pass at some scope produced `n ≥ 2`
    ///   equally-specific candidates. No further scopes consulted.
    /// - [`ResolveOutcome::Deferred`]: nothing matched anywhere on the chain, but `expr`
    ///   contains at least one nested `Expression` / `ListLiteral` / `DictLiteral` part —
    ///   eagerly evaluating those subs may produce a `Future(_)` that matches a typed slot
    ///   the bare expression couldn't. The scheduler falls through to its eager-sub loop
    ///   on this variant. Covers shapes like `((deep_call) + 1)` where a typed `+`
    ///   overload only matches after `deep_call` resolves.
    /// - [`ResolveOutcome::Unmatched`]: no match anywhere, and no eager parts to wait on
    ///   either — a real dispatch failure the caller surfaces as an error.
    pub fn resolve_dispatch<'e>(&'a self, expr: &KExpression<'e>) -> ResolveOutcome<'a> {
        let key = expr.untyped_key();
        let mut current: Option<&'a Scope<'a>> = Some(self);
        while let Some(scope) = current {
            let functions_guard = scope.bindings().functions();
            if let Some(bucket) = functions_guard.get(&key) {
                // Strict pass within this scope.
                let strict: Vec<&'a KFunction<'a>> = bucket
                    .iter()
                    .copied()
                    .filter(|f| f.signature.matches(expr))
                    .collect();
                let strict_sigs: Vec<&crate::runtime::machine::model::types::ExpressionSignature> =
                    strict.iter().map(|f| &f.signature).collect();
                match crate::runtime::machine::model::types::ExpressionSignature::most_specific(&strict_sigs)
                {
                    Some(i) => {
                        let picked = strict[i];
                        return ResolveOutcome::Resolved(build_resolved(picked, expr));
                    }
                    None if !strict.is_empty() => {
                        // Tie inside this scope — surface ambiguity rather than fall through.
                        return ResolveOutcome::Ambiguous(strict.len());
                    }
                    None => {}
                }
                // Tentative (auto-wrap) pass within the same scope.
                let tentative: Vec<&'a KFunction<'a>> = bucket
                    .iter()
                    .copied()
                    .filter(|f| f.accepts_for_wrap(expr))
                    .collect();
                let tentative_sigs: Vec<&crate::runtime::machine::model::types::ExpressionSignature> =
                    tentative.iter().map(|f| &f.signature).collect();
                match crate::runtime::machine::model::types::ExpressionSignature::most_specific(
                    &tentative_sigs,
                ) {
                    Some(i) => {
                        let picked = tentative[i];
                        return ResolveOutcome::Resolved(build_resolved(picked, expr));
                    }
                    None if !tentative.is_empty() => {
                        // Tentative-pass ambiguity: the wrap pass mustn't speculatively
                        // transform an expression with multiple equally-loose candidates.
                        // Fall through to `outer` rather than surfacing `Ambiguous` — the
                        // tentative pass is already a relaxation, and an outer scope's
                        // strict pick is the stronger signal.
                    }
                    None => {}
                }
            }
            // Drop the borrow before recursing into `outer` — the outer scope's bucket
            // lookup also calls `bindings().functions()`, and `RefCell` borrows in a
            // shared chain need explicit release because NLL would not drop the inner
            // guard early enough on its own.
            drop(functions_guard);
            current = scope.outer;
        }
        // Nothing matched on the chain. Distinguish a flat-unbound shape from one whose
        // dispatch can't pick *yet* because nested subs need to evaluate first — the
        // scheduler's eager-sub loop will rebuild with `Future(_)` parts and re-dispatch.
        if expr_has_eager_part(expr) {
            ResolveOutcome::Deferred
        } else {
            ResolveOutcome::Unmatched
        }
    }
}

/// True iff `expr` carries any `Expression` / `ListLiteral` / `DictLiteral` part — the
/// shapes the scheduler's eager loop would schedule as sub-Dispatches. Drives the
/// [`ResolveOutcome::Deferred`] vs [`ResolveOutcome::Unmatched`] split: a nested-call shape
/// like `((deep_call) + 1)` defers (today's behavior); a flat unbound name `nope` is
/// unmatched.
fn expr_has_eager_part(expr: &KExpression<'_>) -> bool {
    expr.parts.iter().any(|p| {
        matches!(
            p,
            crate::ast::ExpressionPart::Expression(_)
                | crate::ast::ExpressionPart::ListLiteral(_)
                | crate::ast::ExpressionPart::DictLiteral(_)
        )
    })
}

/// Pack a picked function + classification + `pre_run`-extracted placeholder name into a
/// [`Resolved`]. The sole producer of the embedded `slots` — disjointness lives in
/// [`KFunction::classify_for_pick`].
fn build_resolved<'a>(
    picked: &'a KFunction<'a>,
    expr: &KExpression<'_>,
) -> Resolved<'a> {
    Resolved {
        function: picked,
        placeholder_name: picked.pre_run.and_then(|extractor| extractor(expr)),
        slots: picked.classify_for_pick(expr),
    }
}

/// A successful resolution: which function was picked, what placeholder name (if any) to
/// install at dispatch time, and the per-slot classification a downstream scheduler driver
/// needs for auto-wrap, replay-park, and eager-sub scheduling. `slots` is held by value —
/// `build_resolved` is the sole producer, so this is the single carrier for the disjoint
/// `(eager_indices | wrap_indices | ref_name_indices)` invariant documented on
/// [`crate::runtime::machine::kfunction::ClassifiedSlots`].
pub struct Resolved<'a> {
    pub function: &'a KFunction<'a>,
    pub placeholder_name: Option<String>,
    pub slots: crate::runtime::machine::kfunction::ClassifiedSlots,
}

/// Outcome of [`Scope::resolve_dispatch`]. See that method's docstring for the meaning of
/// each variant. The `Resolved | Ambiguous | Deferred | Unmatched` split is the
/// load-bearing typing — the scheduler's dispatch driver matches on it directly to choose
/// between immediate bind, ambiguity error, eager-sub scheduling, and dispatch-failed
/// error.
pub enum ResolveOutcome<'a> {
    Resolved(Resolved<'a>),
    Ambiguous(usize),
    Deferred,
    Unmatched,
}
