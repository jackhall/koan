use std::cell::RefCell;
use std::io::Write;

use crate::machine::model::ast::KExpression;

use crate::machine::core::kfunction::{ArgumentBundle, KFunction, NodeId};
use crate::machine::model::values::KObject;
use super::arena::RuntimeArena;
use super::bindings::{ApplyOutcome, Bindings};
use super::kerror::KError;
use super::pending::PendingQueue;
use super::scope_id::ScopeId;

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
    /// Position-independent identity. Minted from [`ScopeId::next`] at construction;
    /// captured into `KType::UserType { scope_id, .. }` / `KType::SignatureBound {
    /// sig_id, .. }` and the corresponding `KObject` schema variants so dispatch on
    /// user-declared types compares ids rather than scope pointers. See
    /// [`crate::machine::core::scope_id`].
    pub id: ScopeId,
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
            id: ScopeId::next(),
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
            id: ScopeId::next(),
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
            id: ScopeId::next(),
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
            id: ScopeId::next(),
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

    /// Iterate `self` and its `outer` chain. Single-source-of-truth for the lexical-
    /// parent walk; previously open-coded as `let mut current = Some(self); while let
    /// Some(s) = current { ...; current = s.outer; }` in five separate methods. Each
    /// step yields a `&Scope<'a>` with the borrow's lifetime (`self.outer` items are
    /// `&'a Scope<'a>`, reborrowed to the shorter outer borrow). Per-step `RefCell`
    /// guards (e.g. `bindings().types()`) taken inside a `find_map` / `find` closure
    /// drop at the closure boundary, so the release-before-recurse discipline
    /// previously commented on `resolve_type` and `resolve_dispatch` is now structural.
    pub fn ancestors(&self) -> impl Iterator<Item = &Scope<'a>> {
        std::iter::once(self).chain(std::iter::successors(self.outer, |s| s.outer))
    }

    /// True iff `self`'s nearest non-`Anonymous` enclosing scope is a SIG decl_scope.
    /// The walk starts at `self` — a SIG-body builtin's body runs against the SIG
    /// decl_scope directly. A non-SIG named scope (`Module`) short-circuits to `false`;
    /// `Anonymous` frames are transparent and the walk continues outward.
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
    /// [`crate::builtins::value_lookup::body_type_expr`].
    ///
    /// Same conditional-defer shape as [`Self::bind_value`] and
    /// [`Self::register_function`]: direct write first, queue through
    /// [`PendingQueue::defer_type`] on borrow conflict. Infallible like the prior
    /// implementation — a name collision at builtin registration is a programming
    /// error, so the [`KError`] from `try_register_type` is dropped.
    pub fn register_type(&self, name: String, ktype: crate::machine::model::types::KType) {
        let kt_ref: &'a crate::machine::model::types::KType = self.arena.alloc_ktype(ktype);
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
    /// Called by [`crate::machine::model::types::resolver::close_type_cycle`] from
    /// inside the elaborator's `Resolution::Placeholder` arm. At that call site no
    /// outer `bindings` borrow is held (the placeholder lookup released its `Ref`
    /// before returning), so a conflict here is a programming error. The
    /// downstream finalize's
    /// [`crate::machine::core::Bindings::try_register_nominal`] idempotent
    /// arm picks up the carrier write against this pre-installed identity.
    pub fn cycle_close_install_identity(
        &self,
        name: String,
        ktype: crate::machine::model::types::KType,
    ) {
        let kt_ref: &'a crate::machine::model::types::KType = self.arena.alloc_ktype(ktype);
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
        kt: crate::machine::model::types::KType,
        obj: &'a KObject<'a>,
    ) -> Result<&'a KObject<'a>, KError> {
        let kt_ref: &'a crate::machine::model::types::KType = self.arena.alloc_ktype(kt);
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
        self.ancestors()
            .find_map(|scope| {
                if let Some(obj) = scope.bindings.data().get(name).copied() {
                    return Some(Resolution::Value(obj));
                }
                scope.bindings.placeholders().get(name).copied().map(Resolution::Placeholder)
            })
            .unwrap_or(Resolution::Unbound)
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
    /// reserved for stage 3's `pending_types` registry). The per-step
    /// [`Bindings::types`] `Ref` is taken inside the `find_map` closure and drops at
    /// the closure boundary, so a deep chain never accumulates live read borrows.
    pub fn resolve_type(&self, name: &str) -> Option<&'a crate::machine::model::types::KType> {
        self.ancestors().find_map(|scope| scope.bindings.types().get(name).copied())
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

