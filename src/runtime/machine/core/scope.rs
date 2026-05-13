use std::cell::{Ref, RefCell};
use std::collections::HashMap;
use std::io::Write;

use crate::ast::{KExpression, TypeExpr, TypeParams};

use crate::runtime::machine::kfunction::{ArgumentBundle, KFunction, NodeId};
use crate::runtime::model::types::UntypedKey;
use crate::runtime::model::values::KObject;
use super::arena::RuntimeArena;
use super::kerror::{KError, KErrorKind};

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

/// A pending re-entrant write — queued when `try_borrow_mut` on `data`/`functions` collides
/// with a borrow held up the call stack, retried by `drain_pending` between scheduler nodes.
/// The variant tag preserves the per-signature dedupe and value/function collision check on
/// retry, which a single shared retry path would skip.
enum PendingWrite<'a> {
    Value { name: String, obj: &'a KObject<'a> },
    Function { name: String, fn_ref: &'a KFunction<'a>, obj: &'a KObject<'a> },
}

/// Façade owning the three co-mutating `RefCell` maps that back every lexical binding:
/// `data` (name → value), `functions` (untyped-signature bucket → overloads), and
/// `placeholders` (name → producer NodeId for dispatch-time forward references).
///
/// The shared private [`Bindings::try_apply`] enforces the dual-map invariant —
/// every `data[name]` entry wrapping a `KFunction` lives in
/// `functions[signature.untyped_key()]` — in one place, and unifies dedupe (`ptr::eq`
/// fast-path then `signatures_exact_equal`) across the LET-binds-FN and `FN`-decl paths.
///
/// Lifetime `'a` matches the arena lifetime of the stored references; the façade itself
/// is embedded by value on [`Scope`] so all interior borrows arbitrate against one another.
pub struct Bindings<'a> {
    data: RefCell<HashMap<String, &'a KObject<'a>>>,
    functions: RefCell<HashMap<UntypedKey, Vec<&'a KFunction<'a>>>>,
    placeholders: RefCell<HashMap<String, NodeId>>,
}

impl<'a> Bindings<'a> {
    pub fn new() -> Self {
        Self {
            data: RefCell::new(HashMap::new()),
            functions: RefCell::new(HashMap::new()),
            placeholders: RefCell::new(HashMap::new()),
        }
    }

    /// Read-only handle for the ~12 read sites in builtins and resolver code. The returned
    /// `Ref<'_, _>` has the lifetime of `&self`, so the usual `for (k, v) in
    /// scope.bindings().data().iter()` pattern extends the temporary through the loop —
    /// same semantics as the prior `RefCell::borrow()` calls.
    pub fn data(&self) -> Ref<'_, HashMap<String, &'a KObject<'a>>> {
        self.data.borrow()
    }

    /// Read-only handle for `resolve_dispatch`'s outer-chain walk and the submission-time
    /// `pre_run` extractor. Same `Ref<'_, _>` semantics as [`Bindings::data`].
    pub fn functions(&self) -> Ref<'_, HashMap<UntypedKey, Vec<&'a KFunction<'a>>>> {
        self.functions.borrow()
    }

    /// Read-only handle for the resolver's placeholder lookup. Same semantics as
    /// [`Bindings::data`].
    pub fn placeholders(&self) -> Ref<'_, HashMap<String, NodeId>> {
        self.placeholders.borrow()
    }

    /// LET-style value bind. Errors `Rebind` if `data[name]` already exists. When `obj`
    /// wraps a `KFunction`, the function is *also* mirrored into the `functions` bucket
    /// keyed by its untyped signature so dispatch finds it — supports `LET f = (FN ...)`
    /// where the bound name doubles as a callable verb.
    ///
    /// `Conflict` outcome means borrow contention (caller queues); `Err` is semantic
    /// rejection (not queued).
    pub fn try_bind_value(
        &self,
        name: &str,
        obj: &'a KObject<'a>,
    ) -> Result<ApplyOutcome, KError> {
        let fn_part = match obj {
            KObject::KFunction(f, _) => Some(*f),
            _ => None,
        };
        self.try_apply(name, obj, fn_part)
    }

    /// FN-style overload registration. Adds `fn_ref` to the `functions` bucket keyed by
    /// its untyped signature, then inserts `obj` into `data[name]`. Errors:
    /// - `DuplicateOverload` if the bucket already holds an exact-signature equal function.
    /// - `Rebind` if `data[name]` holds a non-function.
    pub fn try_register_function(
        &self,
        name: &str,
        fn_ref: &'a KFunction<'a>,
        obj: &'a KObject<'a>,
    ) -> Result<ApplyOutcome, KError> {
        self.try_apply(name, obj, Some(fn_ref))
    }

    /// Install a dispatch-time placeholder for `name` -> producer slot `idx`.
    ///
    /// Lenient when `data[name]` already holds a `KObject::KFunction`: silent no-op.
    /// Forward references resolve through the existing function value; a new FN overload
    /// joins the per-signature bucket on finalize without consumers needing to park.
    ///
    /// Errors `Rebind` if `data[name]` holds a non-function or if `placeholders[name]`
    /// already maps to a *different* `NodeId`. Idempotent if re-entered with the same
    /// `NodeId`.
    ///
    /// Unlike [`Bindings::try_bind_value`] and [`Bindings::try_register_function`], this
    /// method panics on borrow conflict rather than returning `Conflict` — placeholder
    /// installs happen at dispatch-time outside the re-entrant-bind hot path, so a conflict
    /// here indicates a programming error, not a queueable retry.
    pub fn try_install_placeholder(&self, name: String, idx: NodeId) -> Result<(), KError> {
        if let Some(existing) = self.data.borrow().get(&name) {
            if matches!(existing, KObject::KFunction(_, _)) {
                return Ok(());
            }
            return Err(KError::new(KErrorKind::Rebind { name }));
        }
        let mut ph = self.placeholders.borrow_mut();
        if let Some(existing) = ph.get(&name).copied() {
            if existing == idx {
                return Ok(());
            }
            return Err(KError::new(KErrorKind::Rebind { name }));
        }
        ph.insert(name, idx);
        Ok(())
    }

    /// Replay another `Bindings`'s `data` through `try_apply` on self. The single entry
    /// point for ascription's bulk-install — snapshots the source `data` into a `Vec` and
    /// releases `src`'s `Ref` before the replay so re-entrant ascription cannot deadlock.
    /// The shared helper re-mirrors `KFunction` entries into `functions` exactly once, so
    /// the caller does not need to walk `src.functions` separately (that's the point of
    /// routing through `try_apply`).
    ///
    /// The replay is order-independent: the dispatch bucket is order-insensitive once
    /// dedupe is applied. Panics on `Conflict` — a fresh `Bindings` should never hit a
    /// borrow conflict against itself, and a conflict here is a programming error.
    pub fn try_bulk_install_from(&self, src: &Bindings<'a>) -> Result<(), KError> {
        let snapshot: Vec<(String, &'a KObject<'a>)> = src
            .data()
            .iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect();
        for (name, obj) in snapshot {
            let fn_part = match obj {
                KObject::KFunction(f, _) => Some(*f),
                _ => None,
            };
            match self.try_apply(&name, obj, fn_part)? {
                ApplyOutcome::Applied => {}
                ApplyOutcome::Conflict => {
                    unreachable!(
                        "try_bulk_install_from on a fresh Bindings should not hit borrow conflict",
                    );
                }
            }
        }
        Ok(())
    }

    /// The shared write path. Borrows `functions` first (only when `fn_part.is_some()`),
    /// then `data` — preserves the non-fn shortcut so `register_type`, LET-body, and
    /// param-binding flows that run under a live outer `functions` borrow stay
    /// deadlock-free. `Conflict` is reserved for borrow contention; semantic errors come
    /// through `Err(KError)`.
    ///
    /// Unified dedupe: when `fn_part.is_some()` and a same-name `data` entry exists, walk
    /// the bucket — `ptr::eq` is silent-success short-circuit (preserves intentional
    /// aliases like `LET g = (f)`), `signatures_exact_equal` is `DuplicateOverload`. Both
    /// `FN`-decl and `LET`-binds-`FN` paths see both rules. This closes the latent gap
    /// where `try_apply_value`'s pointer-only dedupe could silently double the bucket on a
    /// structurally identical but pointer-distinct re-bind.
    fn try_apply(
        &self,
        name: &str,
        obj: &'a KObject<'a>,
        fn_part: Option<&'a KFunction<'a>>,
    ) -> Result<ApplyOutcome, KError> {
        // Borrow `functions` first only when there is a function-side mirror to write —
        // skipping otherwise preserves the non-fn shortcut documented above.
        let mut functions_handle = if fn_part.is_some() {
            match self.functions.try_borrow_mut() {
                Ok(g) => Some(g),
                Err(_) => return Ok(ApplyOutcome::Conflict),
            }
        } else {
            None
        };
        let mut data = match self.data.try_borrow_mut() {
            Ok(d) => d,
            Err(_) => return Ok(ApplyOutcome::Conflict),
        };
        // Semantic rejection on existing `data[name]`. The `fn_part.is_some()` and entry-
        // already-`KFunction` case falls through to bucket dedupe (overload-add path).
        if let Some(existing) = data.get(name) {
            match fn_part {
                None => return Err(KError::new(KErrorKind::Rebind { name: name.to_string() })),
                Some(_) => {
                    if !matches!(existing, KObject::KFunction(_, _)) {
                        return Err(KError::new(KErrorKind::Rebind { name: name.to_string() }));
                    }
                }
            }
        }
        if let (Some(f_ref), Some(functions)) = (fn_part, functions_handle.as_mut()) {
            let key = f_ref.signature.untyped_key();
            let bucket = functions.entry(key).or_default();
            let mut already_present = false;
            for existing in bucket.iter() {
                if std::ptr::eq(*existing, f_ref) {
                    already_present = true;
                    break;
                }
                if signatures_exact_equal(&existing.signature, &f_ref.signature) {
                    return Err(KError::new(KErrorKind::DuplicateOverload {
                        name: name.to_string(),
                        signature: existing.summarize(),
                    }));
                }
            }
            if !already_present {
                bucket.push(f_ref);
            }
        }
        data.insert(name.to_string(), obj);
        drop(data);
        drop(functions_handle);
        // Best-effort placeholder clear — `try_borrow_mut().ok()` tolerates a caller
        // holding a placeholder borrow up the stack. Promoting this to `borrow_mut()`
        // would panic for a previously-tolerated case.
        if let Ok(mut ph) = self.placeholders.try_borrow_mut() {
            ph.remove(name);
        }
        Ok(ApplyOutcome::Applied)
    }
}

impl<'a> Default for Bindings<'a> {
    fn default() -> Self {
        Self::new()
    }
}

/// Lexical environment. `functions` (inside [`Bindings`]) buckets overloads by their
/// *untyped signature* (token shape with slot types erased) so dispatch can pick between
/// same-shape overloads by `KType` specificity. Only the root scope holds a writer in
/// `out`; child scopes have `None` and `write_out` walks `outer` to find one.
///
/// All mutable binding state lives in the embedded [`Bindings`] façade (interior-mutable
/// `RefCell`s), so a `&'a Scope<'a>` can be shared across scheduler nodes while builtins
/// still mutate through it. The `pending` queue and `out` writer stay on `Scope` because
/// the queue's retry calls into `Scope`'s shim methods anyway; lifting it is the next
/// roadmap item (`pending-queue-facade`).
pub struct Scope<'a> {
    pub outer: Option<&'a Scope<'a>>,
    /// All three binding maps live here — public so test fixtures can read them as
    /// `scope.bindings.data()` etc., but writes only flow through the methods.
    pub bindings: Bindings<'a>,
    pub out: RefCell<Option<Box<dyn Write + 'a>>>,
    pub arena: &'a RuntimeArena,
    /// Writes that hit a borrow conflict at `bind_value` / `register_function` time.
    /// Drained between dispatch nodes by `drain_pending`; direct writes bypass the queue.
    pending: RefCell<Vec<PendingWrite<'a>>>,
    /// Lexical-context label set at construction by `child_under_named` (e.g. `"MODULE Foo"`,
    /// `"SIG OrderedSig"`); empty for run-root and ordinary call frames. Record-only;
    /// reserved for future diagnostics.
    pub name: String,
}

impl<'a> Scope<'a> {
    pub fn run_root(arena: &'a RuntimeArena, outer: Option<&'a Scope<'a>>, out: Box<dyn Write + 'a>) -> Self {
        Self {
            outer,
            bindings: Bindings::new(),
            out: RefCell::new(Some(out)),
            arena,
            pending: RefCell::new(Vec::new()),
            name: String::new(),
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
            pending: RefCell::new(Vec::new()),
            name: String::new(),
        }
    }

    /// Like `child_under` but stamps the scope's `name` with a lexical-context label.
    pub fn child_under_named(outer: &'a Scope<'a>, name: String) -> Scope<'a> {
        Scope {
            outer: Some(outer),
            bindings: Bindings::new(),
            out: RefCell::new(None),
            arena: outer.arena,
            pending: RefCell::new(Vec::new()),
            name,
        }
    }

    /// Borrow the embedded [`Bindings`] façade. Internal callers that need direct access to
    /// the unified write path (e.g. ascription's `try_bulk_install_from`) reach for this;
    /// the shim methods below cover the common cases.
    pub fn bindings(&self) -> &Bindings<'a> {
        &self.bindings
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
                self.pending.borrow_mut().push(PendingWrite::Value { name, obj });
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
                self.pending.borrow_mut().push(PendingWrite::Function {
                    name,
                    fn_ref,
                    obj,
                });
                Ok(())
            }
        }
    }

    /// Register `name` as a type-valued binding in this scope. The binding lives in
    /// [`Bindings::data`] as a `KObject::TypeExprValue` carrying the bare-leaf surface form
    /// (`TypeExpr { name, params: None }`); the type resolver re-elaborates it to a
    /// [`crate::runtime::model::types::KType`] on lookup via
    /// [`crate::runtime::model::types::KType::from_type_expr`], which falls back to
    /// [`crate::runtime::model::types::KType::from_name`] for the parameterless leaf.
    ///
    /// This is the dual of [`Self::register_function`] for the type half of the binding
    /// surface — the call site that would otherwise reach into `Bindings::data` directly to
    /// seed builtin type names goes through here so the borrow / arena / pending-defer
    /// plumbing matches the function path. The `_ktype` parameter mirrors how
    /// `register_function` carries the function value: it documents what the binding
    /// resolves to and guards against drift between the registered name and the resolver's
    /// `from_name` mapping (debug-asserted), even though storage is the surface form.
    ///
    /// Infallible like the function-side `register_builtin` wrapper: a name collision at
    /// builtin registration is a programming error, so the [`KErrorKind::Rebind`] returned
    /// by the underlying `bind_value` is dropped. Per-call-site error handling would just
    /// bury the bug.
    pub fn register_type(&self, name: String, _ktype: crate::runtime::model::types::KType) {
        debug_assert_eq!(
            crate::runtime::model::types::KType::from_name(&name),
            Some(_ktype.clone()),
            "register_type({name:?}, {:?}): name does not match KType::from_name",
            _ktype,
        );
        let arena = self.arena;
        let te = TypeExpr { name: name.clone(), params: TypeParams::None };
        let obj: &'a KObject<'a> = arena.alloc_object(KObject::TypeExprValue(te));
        let _ = self.bind_value(name, obj);
    }

    /// Apply queued writes between dispatch nodes. Items that still hit a borrow conflict
    /// stay queued (eventually-consistent, not guaranteed-empty after one call). Semantic
    /// failures on retry are silently dropped — there is no caller to surface the error to.
    pub fn drain_pending(&self) {
        if self.pending.borrow().is_empty() {
            return;
        }
        let pending = std::mem::take(&mut *self.pending.borrow_mut());
        let mut still_pending: Vec<PendingWrite<'a>> = Vec::new();
        for item in pending {
            match item {
                PendingWrite::Value { name, obj } => {
                    match self.bindings.try_bind_value(&name, obj) {
                        Ok(ApplyOutcome::Applied) => {}
                        Ok(ApplyOutcome::Conflict) => {
                            still_pending.push(PendingWrite::Value { name, obj });
                        }
                        Err(_) => {}
                    }
                }
                PendingWrite::Function { name, fn_ref, obj } => {
                    match self.bindings.try_register_function(&name, fn_ref, obj) {
                        Ok(ApplyOutcome::Applied) => {}
                        Ok(ApplyOutcome::Conflict) => {
                            still_pending.push(PendingWrite::Function { name, fn_ref, obj });
                        }
                        Err(_) => {}
                    }
                }
            }
        }
        if !still_pending.is_empty() {
            self.pending.borrow_mut().extend(still_pending);
        }
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
                let strict_sigs: Vec<&crate::runtime::model::types::ExpressionSignature> =
                    strict.iter().map(|f| &f.signature).collect();
                match crate::runtime::model::types::ExpressionSignature::most_specific(&strict_sigs)
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
                let tentative_sigs: Vec<&crate::runtime::model::types::ExpressionSignature> =
                    tentative.iter().map(|f| &f.signature).collect();
                match crate::runtime::model::types::ExpressionSignature::most_specific(
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

/// `Conflict` is reserved for borrow contention; semantic errors come through `Err(KError)`.
pub enum ApplyOutcome {
    Applied,
    Conflict,
}

/// Structural equality on shape + per-Argument `KType` + return type. Independent of
/// `Argument::name` — two overloads with matching shape and types collide for dispatch
/// regardless of parameter naming.
fn signatures_exact_equal(
    a: &crate::runtime::model::types::ExpressionSignature,
    b: &crate::runtime::model::types::ExpressionSignature,
) -> bool {
    use crate::runtime::model::types::SignatureElement;
    if a.return_type != b.return_type {
        return false;
    }
    if a.elements.len() != b.elements.len() {
        return false;
    }
    a.elements.iter().zip(b.elements.iter()).all(|(x, y)| match (x, y) {
        (SignatureElement::Keyword(s), SignatureElement::Keyword(t)) => s == t,
        (SignatureElement::Argument(ax), SignatureElement::Argument(ay)) => ax.ktype == ay.ktype,
        _ => false,
    })
}

#[cfg(test)]
mod tests {
    use super::{Resolution, RuntimeArena, Scope};
    use crate::runtime::builtins::test_support::run_root_bare;
    use crate::runtime::machine::kfunction::{Body, KFunction, NodeId};
    use crate::runtime::model::types::{Argument, ExpressionSignature, KType, SignatureElement};
    use crate::runtime::model::values::KObject;

    fn unit_signature() -> ExpressionSignature {
        ExpressionSignature {
            return_type: KType::Any,
            elements: vec![SignatureElement::Keyword("FOO".into())],
        }
    }

    fn body_no_op<'a>(
        _scope: &'a Scope<'a>,
        _sched: &mut dyn crate::runtime::machine::kfunction::SchedulerHandle<'a>,
        _bundle: crate::runtime::machine::kfunction::ArgumentBundle<'a>,
    ) -> crate::runtime::machine::kfunction::BodyResult<'a> {
        crate::runtime::machine::kfunction::BodyResult::Value(_scope.arena.alloc_object(KObject::Null))
    }

    /// Snapshot-iteration semantics: a re-entrant `bind_value` queues silently and only
    /// becomes visible after `drain_pending`; the held iteration sees the pre-write state.
    #[test]
    fn add_during_active_data_borrow_queues_and_drains() {
        let arena = RuntimeArena::new();
        let scope = run_root_bare(&arena);
        let pre = arena.alloc_object(KObject::Number(1.0));
        scope.bind_value("pre".to_string(), pre).unwrap();

        let new_entry = arena.alloc_object(KObject::Number(2.0));
        {
            let snapshot = scope.bindings().data();
            assert!(snapshot.contains_key("pre"));
            scope.bind_value("during".to_string(), new_entry).unwrap();
            assert!(!snapshot.contains_key("during"));
        }
        assert!(scope.bindings().data().get("during").is_none());
        scope.drain_pending();
        let after = scope.bindings().data();
        assert!(matches!(after.get("during"), Some(KObject::Number(n)) if *n == 2.0));
    }

    #[test]
    fn bind_value_errors_on_same_scope_rebind() {
        let arena = RuntimeArena::new();
        let scope = run_root_bare(&arena);
        let v1 = arena.alloc_object(KObject::Number(1.0));
        let v2 = arena.alloc_object(KObject::Number(2.0));
        scope.bind_value("x".to_string(), v1).unwrap();
        let err = scope.bind_value("x".to_string(), v2).unwrap_err();
        match &err.kind {
            crate::runtime::machine::core::KErrorKind::Rebind { name } => assert_eq!(name, "x"),
            _ => panic!("expected Rebind, got {err}"),
        }
    }

    #[test]
    fn bind_value_allows_shadowing_in_child_scope() {
        let arena = RuntimeArena::new();
        let outer = run_root_bare(&arena);
        let v1 = arena.alloc_object(KObject::Number(1.0));
        outer.bind_value("x".to_string(), v1).unwrap();
        let inner = arena.alloc_scope(outer.child_for_call());
        let v2 = arena.alloc_object(KObject::Number(2.0));
        inner.bind_value("x".to_string(), v2).unwrap();
        assert!(matches!(inner.lookup("x"), Some(KObject::Number(n)) if *n == 2.0));
        assert!(matches!(outer.lookup("x"), Some(KObject::Number(n)) if *n == 1.0));
    }

    #[test]
    fn register_function_dedupes_exact_signature() {
        let arena = RuntimeArena::new();
        let scope = run_root_bare(&arena);
        let f1 = arena.alloc_function(KFunction::new(unit_signature(), Body::Builtin(body_no_op), scope));
        let obj1 = arena.alloc_object(KObject::KFunction(f1, None));
        scope.register_function("FOO".to_string(), f1, obj1).unwrap();
        let f2 = arena.alloc_function(KFunction::new(unit_signature(), Body::Builtin(body_no_op), scope));
        let obj2 = arena.alloc_object(KObject::KFunction(f2, None));
        let err = scope.register_function("FOO".to_string(), f2, obj2).unwrap_err();
        assert!(
            matches!(&err.kind, crate::runtime::machine::core::KErrorKind::DuplicateOverload { name, .. } if name == "FOO"),
            "expected DuplicateOverload, got {err}",
        );
    }

    /// Companion to `register_function_dedupes_exact_signature`: routing a structurally
    /// identical but pointer-distinct `KFunction` through the LET path
    /// (`bind_value(KObject::KFunction(...))`) must also trip `DuplicateOverload`. Pre-
    /// façade the LET path only dedup'd by `ptr::eq`, so a fresh-arena-allocated function
    /// with matching signature silently doubled the bucket. The unified `try_apply` closes
    /// this gap. Uses a different name from the prior FN so the test focuses on bucket
    /// dedupe rather than the `Rebind`-on-existing-name path.
    #[test]
    fn bind_value_with_kfunction_dedupes_exact_signature_with_existing_fn() {
        let arena = RuntimeArena::new();
        let scope = run_root_bare(&arena);
        let f1 = arena.alloc_function(KFunction::new(unit_signature(), Body::Builtin(body_no_op), scope));
        let obj1 = arena.alloc_object(KObject::KFunction(f1, None));
        scope.register_function("FOO".to_string(), f1, obj1).unwrap();
        // Pointer-distinct, structurally identical signature — fresh arena allocation.
        let f2 = arena.alloc_function(KFunction::new(unit_signature(), Body::Builtin(body_no_op), scope));
        let obj2 = arena.alloc_object(KObject::KFunction(f2, None));
        let err = scope
            .bind_value("OTHER_NAME".to_string(), obj2)
            .unwrap_err();
        assert!(
            matches!(&err.kind, crate::runtime::machine::core::KErrorKind::DuplicateOverload { name, .. } if name == "OTHER_NAME"),
            "expected DuplicateOverload from LET path, got {err}",
        );
    }

    /// The `ptr::eq` fast-path still allows intentional aliasing: `LET g = (f)` where the
    /// same `&KFunction` is bound under a second name must succeed without
    /// `DuplicateOverload`. This pins the rule that the bucket dedupe is silent-success on
    /// pointer-equal entries and structural-rejection only on pointer-distinct ones.
    #[test]
    fn bind_value_with_kfunction_pointer_equal_alias_no_op() {
        let arena = RuntimeArena::new();
        let scope = run_root_bare(&arena);
        let f = arena.alloc_function(KFunction::new(unit_signature(), Body::Builtin(body_no_op), scope));
        let obj1 = arena.alloc_object(KObject::KFunction(f, None));
        let obj2 = arena.alloc_object(KObject::KFunction(f, None));
        scope.bind_value("FIRST".to_string(), obj1).unwrap();
        // Re-binding under a *different* name with the same `&KFunction` pointer — the
        // intentional-alias case. Must succeed.
        scope.bind_value("ALIAS".to_string(), obj2).unwrap();
    }

    #[test]
    fn register_function_allows_overload_with_different_arg_types() {
        let arena = RuntimeArena::new();
        let scope = run_root_bare(&arena);
        let sig_num = ExpressionSignature {
            return_type: KType::Any,
            elements: vec![
                SignatureElement::Keyword("BAR".into()),
                SignatureElement::Argument(Argument { name: "v".into(), ktype: KType::Number }),
            ],
        };
        let sig_str = ExpressionSignature {
            return_type: KType::Any,
            elements: vec![
                SignatureElement::Keyword("BAR".into()),
                SignatureElement::Argument(Argument { name: "v".into(), ktype: KType::Str }),
            ],
        };
        let f1 = arena.alloc_function(KFunction::new(sig_num, Body::Builtin(body_no_op), scope));
        let f2 = arena.alloc_function(KFunction::new(sig_str, Body::Builtin(body_no_op), scope));
        let obj1 = arena.alloc_object(KObject::KFunction(f1, None));
        let obj2 = arena.alloc_object(KObject::KFunction(f2, None));
        scope.register_function("BAR".to_string(), f1, obj1).unwrap();
        scope.register_function("BAR".to_string(), f2, obj2).unwrap();
    }

    #[test]
    fn register_function_errors_on_function_value_collision() {
        let arena = RuntimeArena::new();
        let scope = run_root_bare(&arena);
        let v = arena.alloc_object(KObject::Number(1.0));
        scope.bind_value("FOO".to_string(), v).unwrap();
        let f = arena.alloc_function(KFunction::new(unit_signature(), Body::Builtin(body_no_op), scope));
        let obj = arena.alloc_object(KObject::KFunction(f, None));
        let err = scope.register_function("FOO".to_string(), f, obj).unwrap_err();
        assert!(
            matches!(&err.kind, crate::runtime::machine::core::KErrorKind::Rebind { name } if name == "FOO"),
            "expected Rebind on function/value collision, got {err}",
        );
    }

    #[test]
    fn resolve_returns_placeholder_when_only_placeholder_exists() {
        let arena = RuntimeArena::new();
        let scope = run_root_bare(&arena);
        scope.install_placeholder("x".to_string(), NodeId(7)).unwrap();
        match scope.resolve("x") {
            Resolution::Placeholder(id) => assert_eq!(id, NodeId(7)),
            _ => panic!("expected Placeholder"),
        }
    }

    #[test]
    fn resolve_stops_at_first_hit_does_not_descend_outer() {
        let arena = RuntimeArena::new();
        let outer = run_root_bare(&arena);
        let v = arena.alloc_object(KObject::Number(1.0));
        outer.bind_value("x".to_string(), v).unwrap();
        let inner = arena.alloc_scope(outer.child_for_call());
        inner.install_placeholder("x".to_string(), NodeId(3)).unwrap();
        match inner.resolve("x") {
            Resolution::Placeholder(id) => assert_eq!(id, NodeId(3)),
            other => panic!(
                "expected Placeholder from inner — outer's Value should not shadow it. Got {}",
                match other {
                    Resolution::Value(_) => "Value",
                    Resolution::Placeholder(_) => "Placeholder",
                    Resolution::Unbound => "Unbound",
                }
            ),
        }
    }

    #[test]
    fn bind_value_clears_own_placeholder() {
        let arena = RuntimeArena::new();
        let scope = run_root_bare(&arena);
        scope.install_placeholder("x".to_string(), NodeId(2)).unwrap();
        let v = arena.alloc_object(KObject::Number(42.0));
        scope.bind_value("x".to_string(), v).unwrap();
        assert!(scope.bindings().placeholders().get("x").is_none());
        assert!(matches!(scope.resolve("x"), Resolution::Value(KObject::Number(n)) if *n == 42.0));
    }

    // -------- resolve_dispatch smoke tests --------

    use super::ResolveOutcome;
    use crate::ast::{ExpressionPart, KExpression, KLiteral};
    use crate::runtime::builtins::register_builtin;
    use crate::runtime::builtins::test_support::{marker, one_slot_sig};
    use crate::runtime::machine::kfunction::{ArgumentBundle, BodyResult, SchedulerHandle};

    fn body_a<'a>(s: &'a Scope<'a>, _h: &mut dyn SchedulerHandle<'a>, _a: ArgumentBundle<'a>) -> BodyResult<'a> { BodyResult::Value(marker(s, "a")) }
    fn body_b<'a>(s: &'a Scope<'a>, _h: &mut dyn SchedulerHandle<'a>, _a: ArgumentBundle<'a>) -> BodyResult<'a> { BodyResult::Value(marker(s, "b")) }

    fn two_slot_sig(a: KType, b: KType) -> ExpressionSignature {
        ExpressionSignature {
            return_type: KType::Any,
            elements: vec![
                SignatureElement::Argument(Argument { name: "a".into(), ktype: a }),
                SignatureElement::Keyword("OP".into()),
                SignatureElement::Argument(Argument { name: "b".into(), ktype: b }),
            ],
        }
    }

    /// Successful pick on an overload registered in the current scope: the carried
    /// `Resolved` exposes the classifier's per-slot indices (here, an Identifier in an
    /// `Any` slot lands in `wrap_indices`).
    #[test]
    fn resolve_returns_resolved_with_classified_indices_for_known_overload() {
        let arena = RuntimeArena::new();
        let scope = run_root_bare(&arena);
        register_builtin(scope, "ONE", one_slot_sig("v", KType::Any), body_a);
        let expr = KExpression { parts: vec![ExpressionPart::Identifier("foo".into())] };
        match scope.resolve_dispatch(&expr) {
            ResolveOutcome::Resolved(r) => {
                assert_eq!(r.slots.wrap_indices, vec![0]);
                assert!(r.slots.ref_name_indices.is_empty());
                assert!(!r.slots.picked_has_pre_run);
            }
            _ => panic!("expected Resolved for known overload"),
        }
    }

    /// Tied strict overloads (`<Number> OP <Any>` vs `<Any> OP <Number>` against `5 OP 7`)
    /// surface as `Ambiguous(2)` at the scope where the tie occurs.
    #[test]
    fn resolve_returns_ambiguous_for_tied_overloads() {
        let arena = RuntimeArena::new();
        let scope = run_root_bare(&arena);
        register_builtin(scope, "NA", two_slot_sig(KType::Number, KType::Any), body_a);
        register_builtin(scope, "AN", two_slot_sig(KType::Any, KType::Number), body_b);
        let expr = KExpression {
            parts: vec![
                ExpressionPart::Literal(KLiteral::Number(5.0)),
                ExpressionPart::Keyword("OP".into()),
                ExpressionPart::Literal(KLiteral::Number(7.0)),
            ],
        };
        match scope.resolve_dispatch(&expr) {
            ResolveOutcome::Ambiguous(n) => assert_eq!(n, 2),
            _ => panic!("expected Ambiguous(2) for tied overloads"),
        }
    }

    /// Inner scope has no matching overload; resolution descends to `outer` and picks
    /// there.
    #[test]
    fn resolve_walks_outer_chain_on_unmatched() {
        let arena = RuntimeArena::new();
        let outer = run_root_bare(&arena);
        register_builtin(outer, "ONE", one_slot_sig("v", KType::Any), body_a);
        let inner = arena.alloc_scope(outer.child_for_call());
        let expr = KExpression { parts: vec![ExpressionPart::Identifier("foo".into())] };
        assert!(matches!(inner.resolve_dispatch(&expr), ResolveOutcome::Resolved(_)));
    }

    /// Inner ambiguity does NOT fall through to `outer`: the outer scope has a
    /// non-ambiguous overload, but resolution still reports Ambiguous from the inner tie.
    #[test]
    fn resolve_does_not_descend_outer_on_inner_ambiguity() {
        let arena = RuntimeArena::new();
        let outer = run_root_bare(&arena);
        register_builtin(outer, "OUTER", two_slot_sig(KType::Number, KType::Number), body_a);
        let inner = arena.alloc_scope(outer.child_for_call());
        register_builtin(inner, "NA", two_slot_sig(KType::Number, KType::Any), body_a);
        register_builtin(inner, "AN", two_slot_sig(KType::Any, KType::Number), body_b);
        let expr = KExpression {
            parts: vec![
                ExpressionPart::Literal(KLiteral::Number(5.0)),
                ExpressionPart::Keyword("OP".into()),
                ExpressionPart::Literal(KLiteral::Number(7.0)),
            ],
        };
        match inner.resolve_dispatch(&expr) {
            ResolveOutcome::Ambiguous(_) => {}
            _ => panic!("inner ambiguity must surface, not fall through to outer's unique overload"),
        }
    }

    /// A pre_run-bearing overload (here a synthetic LET-like binder) populates
    /// `placeholder_name` from its extractor.
    #[test]
    fn resolve_carries_placeholder_name_for_pre_run_function() {
        use crate::runtime::builtins::register_builtin_with_pre_run;
        fn name_extractor(expr: &KExpression<'_>) -> Option<String> {
            // Mirror LET's shape: expression's 2nd part is the binder Identifier.
            match expr.parts.get(1) {
                Some(ExpressionPart::Identifier(n)) => Some(n.clone()),
                _ => None,
            }
        }
        let arena = RuntimeArena::new();
        let scope = run_root_bare(&arena);
        let sig = ExpressionSignature {
            return_type: KType::Any,
            elements: vec![
                SignatureElement::Keyword("LETLIKE".into()),
                SignatureElement::Argument(Argument { name: "n".into(), ktype: KType::Identifier }),
                SignatureElement::Keyword("=".into()),
                SignatureElement::Argument(Argument { name: "v".into(), ktype: KType::Any }),
            ],
        };
        register_builtin_with_pre_run(scope, "LETLIKE", sig, body_a, Some(name_extractor));
        let expr = KExpression {
            parts: vec![
                ExpressionPart::Keyword("LETLIKE".into()),
                ExpressionPart::Identifier("foo".into()),
                ExpressionPart::Keyword("=".into()),
                ExpressionPart::Literal(KLiteral::Number(1.0)),
            ],
        };
        match scope.resolve_dispatch(&expr) {
            ResolveOutcome::Resolved(r) => {
                assert_eq!(r.placeholder_name.as_deref(), Some("foo"));
                assert!(r.slots.picked_has_pre_run);
            }
            _ => panic!("expected Resolved with placeholder_name"),
        }
    }

    /// The tentative pass only fires when strict picked nothing at the same scope.
    /// Register only a `<Identifier>` overload; calling with a `Number` literal must miss
    /// strictly *and* tentatively (Literal is not a bare name), giving Unmatched at
    /// run-root.
    #[test]
    fn resolve_tentative_falls_back_only_when_strict_empty() {
        let arena = RuntimeArena::new();
        let scope = run_root_bare(&arena);
        register_builtin(scope, "ONE_ID", one_slot_sig("v", KType::Identifier), body_a);
        let expr = KExpression { parts: vec![ExpressionPart::Literal(KLiteral::Number(5.0))] };
        assert!(matches!(scope.resolve_dispatch(&expr), ResolveOutcome::Unmatched));
    }

    /// A nested-Expression shape `((deep_call) + 1)` returns `Deferred`: the typed `+`
    /// overload doesn't strictly match (Expression in Number slot) and doesn't tentatively
    /// match either (Expression isn't a bare name), but eager evaluation of `(deep_call)`
    /// may produce a `Future(Number)` that the post-Bind re-dispatch picks. Distinct from
    /// `Unmatched` — the scheduler falls through to its eager-sub loop on `Deferred`.
    #[test]
    fn resolve_returns_deferred_for_nested_expression_in_typed_slot() {
        let arena = RuntimeArena::new();
        let scope = run_root_bare(&arena);
        register_builtin(scope, "PLUS", two_slot_sig(KType::Number, KType::Number), body_a);
        let inner = KExpression {
            parts: vec![ExpressionPart::Identifier("deep_call".into())],
        };
        let expr = KExpression {
            parts: vec![
                ExpressionPart::Expression(Box::new(inner)),
                ExpressionPart::Keyword("OP".into()),
                ExpressionPart::Literal(KLiteral::Number(1.0)),
            ],
        };
        assert!(matches!(scope.resolve_dispatch(&expr), ResolveOutcome::Deferred));
    }

    // -------- unit-level dispatch tests on `resolve_dispatch` --------
    //
    // Cover overload-resolution behaviors at the `resolve_dispatch` boundary. The
    // end-to-end variants that drive `Scheduler::execute` live with the rest of the
    // scheduler integration tests at `execute::scheduler::tests`.

    use crate::runtime::builtins::default_scope;

    fn body_number_any<'a>(s: &'a Scope<'a>, _h: &mut dyn SchedulerHandle<'a>, _a: ArgumentBundle<'a>) -> BodyResult<'a> { BodyResult::Value(marker(s, "number_any")) }
    fn body_any_number<'a>(s: &'a Scope<'a>, _h: &mut dyn SchedulerHandle<'a>, _a: ArgumentBundle<'a>) -> BodyResult<'a> { BodyResult::Value(marker(s, "any_number")) }

    /// Parent owns the LET builtin; child has no functions of its own. `resolve_dispatch`
    /// against the child must climb to the parent.
    #[test]
    fn resolve_walks_outer_chain_to_find_builtin() {
        let arena = RuntimeArena::new();
        let outer = default_scope(&arena, Box::new(std::io::sink()));
        let inner = arena.alloc_scope(outer.child_for_call());

        let expr = KExpression {
            parts: vec![
                ExpressionPart::Keyword("LET".into()),
                ExpressionPart::Identifier("x".into()),
                ExpressionPart::Keyword("=".into()),
                ExpressionPart::Literal(KLiteral::Number(1.0)),
            ],
        };

        assert!(
            matches!(inner.resolve_dispatch(&expr), ResolveOutcome::Resolved(_)),
            "child scope should inherit LET from outer",
        );
    }

    /// No overload anywhere on the chain, and no nested eager parts → `Unmatched`.
    #[test]
    fn resolve_dispatch_with_no_outer_and_no_match_is_unmatched() {
        let arena = RuntimeArena::new();
        let scope = run_root_bare(&arena);
        let expr = KExpression {
            parts: vec![ExpressionPart::Identifier("nope".into())],
        };
        assert!(matches!(scope.resolve_dispatch(&expr), ResolveOutcome::Unmatched));
    }

    /// `<Number> OP <Any>` vs `<Any> OP <Number>` against `5 OP 7` are incomparable: each is
    /// more specific in one slot and less in the other. `resolve_dispatch` reports
    /// `Ambiguous`; the integration path surfaces the same error via Scheduler::execute.
    #[test]
    fn dispatch_errors_on_ambiguous_overlap() {
        let arena = RuntimeArena::new();
        let scope = run_root_bare(&arena);
        register_builtin(scope, "number_any", two_slot_sig(KType::Number, KType::Any), body_number_any);
        register_builtin(scope, "any_number", two_slot_sig(KType::Any, KType::Number), body_any_number);

        let expr = KExpression {
            parts: vec![
                ExpressionPart::Literal(KLiteral::Number(5.0)),
                ExpressionPart::Keyword("OP".into()),
                ExpressionPart::Literal(KLiteral::Number(7.0)),
            ],
        };
        assert!(
            matches!(scope.resolve_dispatch(&expr), ResolveOutcome::Ambiguous(_)),
            "equally-specific overloads should produce an Ambiguous outcome",
        );
    }

    /// Ambiguous shape (two equally-specific overloads matching) surfaces as
    /// `ResolveOutcome::Ambiguous` — the wrap pass mustn't speculatively transform an
    /// ambiguous expression. Semantics sharpen vs. today's `shape_pick → None`: that arm
    /// collapsed ambiguity and no-match into one variant; the new surface separates them.
    #[test]
    fn resolve_returns_ambiguous_for_overlap_that_shape_pick_returned_none_for() {
        let arena = RuntimeArena::new();
        let scope = run_root_bare(&arena);
        register_builtin(scope, "OP_NA", two_slot_sig(KType::Number, KType::Any), body_number_any);
        register_builtin(scope, "OP_AN", two_slot_sig(KType::Any, KType::Number), body_any_number);
        let expr = KExpression {
            parts: vec![
                ExpressionPart::Literal(KLiteral::Number(5.0)),
                ExpressionPart::Keyword("OP".into()),
                ExpressionPart::Literal(KLiteral::Number(7.0)),
            ],
        };
        assert!(
            matches!(scope.resolve_dispatch(&expr), ResolveOutcome::Ambiguous(_)),
            "ambiguous overlap → Ambiguous",
        );
    }
}
