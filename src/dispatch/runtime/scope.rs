use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Write;

use crate::parse::kexpression::KExpression;

use crate::dispatch::kfunction::{ArgumentBundle, KFunction, NodeId};
use crate::dispatch::types::UntypedKey;
use crate::dispatch::values::KObject;
use super::arena::RuntimeArena;
use super::kerror::{KError, KErrorKind};

/// A function call that has been resolved but not yet executed: the original parsed expression,
/// the chosen `KFunction`, and the `ArgumentBundle` produced by `KFunction::bind`. Carried
/// inside `KObject::KTask` and is the unit of deferred work in the dispatch system.
pub struct KFuture<'a> {
    pub parsed: KExpression<'a>,
    pub function: &'a KFunction<'a>,
    pub bundle: ArgumentBundle<'a>,
}

impl<'a> KFuture<'a> {
    /// Recursive clone. The `function` reference is shared (KFunctions are arena-allocated
    /// and immutable); `parsed` clones the AST; `bundle` deep-clones each argument value into
    /// fresh `Rc`s so the clone is independent of the original.
    pub fn deep_clone(&self) -> KFuture<'a> {
        KFuture {
            parsed: self.parsed.clone(),
            function: self.function,
            bundle: self.bundle.deep_clone(),
        }
    }
}

/// Result of `Scope::resolve`. `Value` is the common case — the name is bound in `data`.
/// `Placeholder` says a binder dispatched the name but its body hasn't run yet — the carried
/// `NodeId` identifies the producer slot the consumer should park on. `Unbound` means neither
/// `data` nor `placeholders` (in any scope on the chain searched so far) carries the name.
///
/// Resolution stops at the first hit: per the dispatch-time-placeholders plan, `bind_value`
/// removes its own placeholder before inserting into `data`, so `data` and `placeholders` never
/// both hold the same name in one scope; the chain walk stops at the first scope that sees
/// either.
pub enum Resolution<'a> {
    Value(&'a KObject<'a>),
    Placeholder(NodeId),
    Unbound,
}

/// A pending re-entrant write. `bind_value` and `register_function` queue here when their
/// `try_borrow_mut` on `data`/`functions` collides with a borrow held up the call stack;
/// `drain_pending` retries each item between scheduler nodes. The split tag picks the right
/// retry path so a queued function registration doesn't accidentally take the value-binding
/// path (which would skip the per-signature dedupe and the value/function collision check).
enum PendingWrite<'a> {
    Value { name: String, obj: &'a KObject<'a> },
    Function { name: String, fn_ref: &'a KFunction<'a>, obj: &'a KObject<'a> },
}

/// Lexical environment. `functions` buckets overloads by their *untyped signature* — the
/// arrangement of fixed tokens and slots with slot types erased — so dispatch can pick
/// between same-shape overloads by `KType` specificity. `out` is pluggable so tests and
/// embedders can capture builtin output instead of routing it to stdout — only the root scope
/// holds a writer; child scopes have `None` and `write_out` walks `outer` to find one.
///
/// All mutable state is interior-mutable (`RefCell`) so a `&'a Scope<'a>` can be shared across
/// scheduler nodes (each per-call body Dispatch holds a borrow of its child scope) while
/// builtins still mutate `data` / `functions` / `out` through it.
pub struct Scope<'a> {
    pub outer: Option<&'a Scope<'a>>,
    pub data: RefCell<HashMap<String, &'a KObject<'a>>>,
    pub functions: RefCell<HashMap<UntypedKey, Vec<&'a KFunction<'a>>>>,
    pub out: RefCell<Option<Box<dyn Write + 'a>>>,
    pub arena: &'a RuntimeArena,
    /// Writes that hit a borrow conflict at `bind_value` / `register_function` time
    /// (data/functions already iterated by some caller up the stack). The scheduler drains
    /// this between dispatch nodes via `drain_pending`. Direct writes (no conflict) bypass
    /// the queue entirely, so the hot path is unchanged. See `try_apply_value` /
    /// `try_apply_function` for the conditional-defer logic.
    pending: RefCell<Vec<PendingWrite<'a>>>,
    /// Dispatch-time name placeholders: a binder's `pre_run` hook installs the binder's
    /// declared name here, mapped to the producer slot's `NodeId`, *before* the binder's
    /// body runs. A consumer that looks up the name while the binder's RHS is still in
    /// flight gets `Resolution::Placeholder(producer_id)` and parks on the producer via the
    /// scheduler's `notify_list` / `pending_deps` machinery. The binder's `bind_value` /
    /// `register_function` removes its own placeholder before inserting into `data` /
    /// `functions`, so post-finalize lookups go straight through the value path.
    pub placeholders: RefCell<HashMap<String, NodeId>>,
    /// Lexical-context label for this scope. Set at construction by `MODULE`-style builtins
    /// (`"MODULE Foo"`, `"SIG OrderedSig"`) via `child_under_named`; empty string for run-root
    /// and call frames whose context isn't worth naming. Currently a record-only field — kept
    /// for future diagnostics that may want to surface the scope chain in error messages.
    pub name: String,
}

impl<'a> Scope<'a> {
    /// Construct a fresh root scope with the given writer and arena. Used by `interpret` to
    /// build the run-root that chains under the program-lifetime `default_scope`.
    pub fn run_root(arena: &'a RuntimeArena, outer: Option<&'a Scope<'a>>, out: Box<dyn Write + 'a>) -> Self {
        Self {
            outer,
            data: RefCell::new(HashMap::new()),
            functions: RefCell::new(HashMap::new()),
            out: RefCell::new(Some(out)),
            arena,
            pending: RefCell::new(Vec::new()),
            placeholders: RefCell::new(HashMap::new()),
            name: String::new(),
        }
    }

    /// Build a child scope parented to `self`, sharing its arena. Used by tests that don't
    /// distinguish lexical from dynamic scoping; `KFunction::invoke` uses `child_under`
    /// instead so the child's `outer` is the FN's *captured* scope, not the call site.
    pub fn child_for_call(&'a self) -> Scope<'a> {
        Self::child_under(self)
    }

    /// Build a child scope with an explicit `outer` pointer. Used by `KFunction::invoke` for
    /// user-defined bodies — `outer` is the FN's captured definition scope (lexical scoping),
    /// not the call site. The child shares `outer`'s arena.
    pub fn child_under(outer: &'a Scope<'a>) -> Scope<'a> {
        Scope {
            outer: Some(outer),
            data: RefCell::new(HashMap::new()),
            functions: RefCell::new(HashMap::new()),
            out: RefCell::new(None),
            arena: outer.arena,
            pending: RefCell::new(Vec::new()),
            placeholders: RefCell::new(HashMap::new()),
            name: String::new(),
        }
    }

    /// Construct a named child scope — used by the `MODULE` and `SIG` builtins so the
    /// resulting scope's `name` records the lexical context (`"MODULE IntOrd"`,
    /// `"SIG OrderedSig"`). Identical to `child_under` otherwise.
    pub fn child_under_named(outer: &'a Scope<'a>, name: String) -> Scope<'a> {
        Scope {
            outer: Some(outer),
            data: RefCell::new(HashMap::new()),
            functions: RefCell::new(HashMap::new()),
            out: RefCell::new(None),
            arena: outer.arena,
            pending: RefCell::new(Vec::new()),
            placeholders: RefCell::new(HashMap::new()),
            name,
        }
    }

    /// Bind a value (LET, STRUCT, UNION, SIG, MODULE) under `name` in this scope. Errors with
    /// `Rebind` if `data` already holds `name` (same-scope rebind is rejected per the decided
    /// rule; cross-scope shadowing remains allowed). On success, removes any matching
    /// placeholder this scope owns — the producer has finalized, so consumers should fall
    /// straight through to the value on the next lookup.
    ///
    /// Conditional-defer: tries the direct mutation first, falls back to the `pending` queue
    /// iff a borrow conflict would otherwise panic (typically a caller up the stack is
    /// iterating `data` and re-entrantly triggers a write). The hot path — no concurrent
    /// borrow — is the same direct insert as before; queued writes are observably deferred
    /// until `drain_pending`.
    pub fn bind_value(&self, name: String, obj: &'a KObject<'a>) -> Result<(), KError> {
        // Same-scope rebind check uses an immediate borrow of `data`. If `data` is already
        // borrowed up-stack, route through `pending` — the drain path will re-check.
        match self.try_apply_value(&name, obj)? {
            ApplyOutcome::Applied => Ok(()),
            ApplyOutcome::Conflict => {
                self.pending.borrow_mut().push(PendingWrite::Value { name, obj });
                Ok(())
            }
        }
    }

    /// Register a function (FN body, `register_builtin`) under `name` in this scope. Adds
    /// `fn_ref` to the `functions` bucket keyed by its untyped signature; errors with
    /// `DuplicateOverload` if the bucket already holds an exact-signature equal function.
    /// Then inserts `obj` into `data[name]` only if `data[name]` is empty or already a
    /// `KObject::KFunction` — a function can't be registered under a name that holds a
    /// non-function value (`Rebind`).
    ///
    /// Removes any matching placeholder this scope owns. Conditional-defer routes to the
    /// `pending` queue on borrow conflict (same shape as `bind_value`).
    pub fn register_function(
        &self,
        name: String,
        fn_ref: &'a KFunction<'a>,
        obj: &'a KObject<'a>,
    ) -> Result<(), KError> {
        match self.try_apply_function(&name, fn_ref, obj)? {
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

    /// Direct-write path for value bindings. `Ok(Applied)` on success, `Ok(Conflict)` on
    /// borrow conflict (caller queues into `pending`), `Err(...)` for the `Rebind` case
    /// — routed straight through to the caller without queuing because the conflict is
    /// semantic, not borrow-based.
    ///
    /// When `obj` is a `KObject::KFunction`, the wrapped function is *also* added to
    /// `self.functions[signature_key]` so a downstream dispatch via the function's keyword
    /// signature finds it. This preserves the closure-escape and `LET f = (FN ...)` shapes
    /// where the LET-bound name doubles as a callable verb (call_by_name's apply emits a
    /// Tail with the function's keyword signature, which then routes through the bucket
    /// lookup). Pointer-equality dedupe in the bucket keeps a re-bind of the same function
    /// from doubling up; structural exact-equal dedupe would over-trigger here because
    /// LET aliasing is intentionally permitted.
    fn try_apply_value(
        &self,
        name: &str,
        obj: &'a KObject<'a>,
    ) -> Result<ApplyOutcome, KError> {
        // If we're binding a function value, take the functions borrow first (matches the
        // borrow order in `try_apply_function` so re-entrant `bind_value` from inside an
        // iteration over `functions` defers cleanly via Conflict).
        let mut functions_handle = if matches!(obj, KObject::KFunction(_, _)) {
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
        if data.contains_key(name) {
            return Err(KError::new(KErrorKind::Rebind { name: name.to_string() }));
        }
        // Mirror the function into the per-signature bucket so a downstream dispatch by the
        // function's signature shape finds it. Pointer-dedupe keeps a re-aliased function
        // (e.g. `LET g = (f)` after `LET f = (FN ...)`) from doubling the bucket entry.
        if let (KObject::KFunction(f, _), Some(functions)) = (obj, functions_handle.as_mut()) {
            let key = f.signature.untyped_key();
            let bucket = functions.entry(key).or_default();
            let f_ref: &'a KFunction<'a> = f;
            if !bucket.iter().any(|existing| std::ptr::eq(*existing, f_ref)) {
                bucket.push(f_ref);
            }
        }
        data.insert(name.to_string(), obj);
        drop(data);
        drop(functions_handle);
        // Remove our own placeholder — the binder we owned has finalized.
        if let Ok(mut ph) = self.placeholders.try_borrow_mut() {
            ph.remove(name);
        }
        Ok(ApplyOutcome::Applied)
    }

    /// Direct-write path for function registrations. Mirrors `try_apply_value` but addresses
    /// the `functions` bucket first and uses signature-based dedupe rather than name-based
    /// rebind. `Err(DuplicateOverload)` for exact-signature collision; `Err(Rebind)` if
    /// `data[name]` already holds a non-function.
    fn try_apply_function(
        &self,
        name: &str,
        fn_ref: &'a KFunction<'a>,
        obj: &'a KObject<'a>,
    ) -> Result<ApplyOutcome, KError> {
        let mut functions = match self.functions.try_borrow_mut() {
            Ok(g) => g,
            Err(_) => return Ok(ApplyOutcome::Conflict),
        };
        let mut data = match self.data.try_borrow_mut() {
            Ok(d) => d,
            Err(_) => return Ok(ApplyOutcome::Conflict),
        };
        let key = fn_ref.signature.untyped_key();
        let bucket = functions.entry(key).or_default();
        // Exact-signature dedupe: a re-registration of the same `KFunction` (same pointer)
        // is a silent no-op; an exact-signature equal but distinct `KFunction` is a
        // `DuplicateOverload` error. Pointer-equality is a fast path for the
        // re-registration case (same arena allocation). Structural equality is checked
        // via `signatures_exact_equal` to catch the actual conflict.
        for existing in bucket.iter() {
            if std::ptr::eq(*existing, fn_ref) {
                // Same KFunction re-registered (e.g. LET aliasing of an existing FN);
                // skip the bucket update and fall through to the data insert (which the
                // value-collision check below handles).
                return apply_function_data_insert(&mut data, name, obj, &self.placeholders);
            }
            if signatures_exact_equal(&existing.signature, &fn_ref.signature) {
                return Err(KError::new(KErrorKind::DuplicateOverload {
                    name: name.to_string(),
                    signature: existing.summarize(),
                }));
            }
        }
        bucket.push(fn_ref);
        apply_function_data_insert(&mut data, name, obj, &self.placeholders)
    }

    /// Apply queued writes. Called by the scheduler after each dispatch node so writes that
    /// queued during a re-entrant borrow become visible by the next node's run. Items that
    /// still hit a borrow conflict (rare — would mean drain itself was re-entered through a
    /// live borrow) stay queued for the next drain attempt; the queue is therefore eventually
    /// consistent rather than guaranteed-empty after one call.
    ///
    /// A pending write whose semantic check fails on retry (`Rebind` / `DuplicateOverload`)
    /// is silently dropped here — there is no caller to surface the error to. Today this is
    /// only reachable via the re-entrant-borrow path, which is itself a corner case used by
    /// the `add_during_active_data_borrow_queues_and_drains` test; the test exercises the
    /// no-conflict happy path.
    pub fn drain_pending(&self) {
        if self.pending.borrow().is_empty() {
            return;
        }
        let pending = std::mem::take(&mut *self.pending.borrow_mut());
        let mut still_pending: Vec<PendingWrite<'a>> = Vec::new();
        for item in pending {
            match item {
                PendingWrite::Value { name, obj } => {
                    match self.try_apply_value(&name, obj) {
                        Ok(ApplyOutcome::Applied) => {}
                        Ok(ApplyOutcome::Conflict) => {
                            still_pending.push(PendingWrite::Value { name, obj });
                        }
                        Err(_) => {
                            // Drop semantic errors on the drain path — see method doc.
                        }
                    }
                }
                PendingWrite::Function { name, fn_ref, obj } => {
                    match self.try_apply_function(&name, fn_ref, obj) {
                        Ok(ApplyOutcome::Applied) => {}
                        Ok(ApplyOutcome::Conflict) => {
                            still_pending.push(PendingWrite::Function { name, fn_ref, obj });
                        }
                        Err(_) => {
                            // Drop semantic errors on the drain path — see method doc.
                        }
                    }
                }
            }
        }
        if !still_pending.is_empty() {
            self.pending.borrow_mut().extend(still_pending);
        }
    }

    /// Look up `name` in this scope, walking the `outer` chain on miss. Returns the bound
    /// `KObject` from the nearest enclosing scope, or `None` if unbound at every level.
    /// Thin wrapper over [`Scope::resolve`] that drops the placeholder distinction —
    /// non-`run_dispatch` callers (value_lookup body, ATTR / type_call / module-resolution
    /// bodies, `lookup_kfunction`) keep using this path. With the §1 / §8 dispatch-time
    /// short-circuits in place, those callers never see a placeholder for an in-flight
    /// binder; they continue to surface `UnboundName` for genuinely unbound names.
    pub fn lookup(&self, name: &str) -> Option<&'a KObject<'a>> {
        match self.resolve(name) {
            Resolution::Value(v) => Some(v),
            Resolution::Placeholder(_) | Resolution::Unbound => None,
        }
    }

    /// Resolve `name` against this scope and the `outer` chain. Returns the first hit:
    /// `Value(obj)` if a scope on the chain has a `data[name]` binding, `Placeholder(id)`
    /// if the same scope has a `placeholders[name]` (and no value), or `Unbound` if neither
    /// `data` nor `placeholders` carries `name` in any scope on the chain.
    ///
    /// Walks `data` first, then `placeholders`, in each scope. **Stops at the first hit** —
    /// once an inner scope has the placeholder, an outer scope's `data` binding for the same
    /// name does NOT shadow it. Per the dispatch-time-placeholders plan, `bind_value`
    /// removes the placeholder before inserting into `data`, so `data` and `placeholders`
    /// never both hold the same name in one scope.
    ///
    /// Used by `run_dispatch` (§1 single-Identifier short-circuit and §8 replay-park) to
    /// detect forward references and park consumers on producers.
    pub fn resolve(&self, name: &str) -> Resolution<'a> {
        if let Some(obj) = self.data.borrow().get(name).copied() {
            return Resolution::Value(obj);
        }
        if let Some(id) = self.placeholders.borrow().get(name).copied() {
            return Resolution::Placeholder(id);
        }
        match self.outer {
            Some(outer) => outer.resolve(name),
            None => Resolution::Unbound,
        }
    }

    /// Install a dispatch-time placeholder for `name`, mapped to producer slot `idx`.
    ///
    /// Lenient when `data[name]` already holds a `KObject::KFunction`: returns `Ok(())`
    /// without installing anything. This accommodates the FN overload model where
    /// `register_function` adds new overloads to the per-signature bucket and reuses the
    /// existing `data[name]` slot — a forward reference to `name` while the new overload is
    /// in flight resolves through the *existing* function (the new overload will simply add
    /// itself to the bucket once its body finalizes; consumers don't need to wait).
    ///
    /// Errors with `Rebind` if `data[name]` holds a non-function or if `placeholders[name]`
    /// already holds a *different* `NodeId` than `idx`. Idempotent for re-entry through
    /// `run_dispatch` from a replay-park: if `placeholders[name]` already maps to the same
    /// `NodeId`, no-op.
    ///
    /// Called by `run_dispatch` via the per-binder `pre_run` hook (§4) before any sub-deps
    /// are scheduled, so a sibling expression that looks up `name` while the binder's RHS is
    /// still in flight finds the placeholder and parks.
    pub fn install_placeholder(&self, name: String, idx: NodeId) -> Result<(), KError> {
        if let Some(existing) = self.data.borrow().get(&name) {
            // Function-bucket-managed name: silently skip the install. The existing function
            // value satisfies forward references; the dispatching binder (FN) adds its own
            // overload to the bucket on finalize.
            if matches!(existing, KObject::KFunction(_, _)) {
                return Ok(());
            }
            return Err(KError::new(KErrorKind::Rebind { name }));
        }
        let mut ph = self.placeholders.borrow_mut();
        if let Some(existing) = ph.get(&name).copied() {
            if existing == idx {
                // Re-entry: the same dispatch slot is reinstalling its own placeholder
                // (only happens on a §8 replay-park re-dispatch). No-op.
                return Ok(());
            }
            return Err(KError::new(KErrorKind::Rebind { name }));
        }
        ph.insert(name, idx);
        Ok(())
    }

    /// Walk the `outer` chain to find the nearest scope that holds a writer, and write `bytes`
    /// to it. Used by PRINT. Child scopes constructed by `child_for_call` have `out: None`,
    /// so writes ascend to the run-root (or to whichever scope the caller stashed the writer
    /// into). Errors from the underlying writer are dropped — same shape as the previous
    /// `let _ = writeln!(scope.out, ...)` call site.
    pub fn write_out(&self, bytes: &[u8]) {
        if let Some(w) = self.out.borrow_mut().as_mut() {
            let _ = w.write_all(bytes);
            return;
        }
        if let Some(outer) = self.outer {
            outer.write_out(bytes);
        }
    }

    /// Resolve `expr` against this scope's functions, walking `outer` on miss so child scopes
    /// inherit from their parents. Ambiguity does *not* fall through to `outer` — the inner
    /// scope had a real conflict, and silently shadowing it would hide it from the author.
    ///
    /// Function-as-value calls (e.g., `LET f = (FN ...)` then `f (args)`) do not live here:
    /// they go through the [`call_by_name`](super::builtins::call_by_name) builtin, whose
    /// signature `[Identifier, KExpression]` matches identifier-leading expressions and
    /// synthesizes a re-dispatchable expression by weaving the looked-up function's keyword
    /// tokens back in.
    pub fn dispatch(&self, expr: KExpression<'a>) -> Result<KFuture<'a>, KError> {
        super::dispatcher::dispatch(self, expr)
    }

    /// Look up `name` in the scope chain and return the bound `KFunction`, or `None` if the
    /// name is unbound or bound to a non-function value. Used by the
    /// [`call_by_name`](super::builtins::call_by_name) builtin to resolve identifier-leading
    /// expressions to the function they should invoke.
    pub fn lookup_kfunction(&self, name: &str) -> Option<&'a KFunction<'a>> {
        match self.lookup(name)? {
            KObject::KFunction(f, _) => Some(*f),
            _ => None,
        }
    }

    /// Find a "lazy candidate" for `expr`: a matching function with at least one
    /// `KType::KExpression` slot bound by an `ExpressionPart::Expression`. Returns the indices
    /// of the *eager* `Expression` parts — the caller schedules those as deps and leaves the
    /// lazy ones in place for the receiving builtin to dispatch itself. Walks `outer` like
    /// `dispatch` does.
    ///
    /// TODO(lazy-list-of-expressions): once user functions exist, `[e1 e2 e3]` will need to
    /// ride into the parent as `KExpression` data rather than be eagerly scheduled. Today
    /// every list-literal element resolves eagerly via `schedule_list_literal`.
    pub fn lazy_candidate(&self, expr: &KExpression<'_>) -> Option<Vec<usize>> {
        super::dispatcher::lazy_candidate(self, expr)
    }

    /// Shape-pick helper: extended `lazy_candidate` that returns a single `ShapePick` carrying
    /// the picked function's eager-Expression indices (the existing lazy-candidate behavior),
    /// the bare-Identifier indices that should be *auto-wrapped* as sub-Dispatches (§7), and
    /// the bare-Identifier indices that name *function references* whose binder may not yet
    /// have finalized (§8 replay-park targets). Returns `None` if no unique candidate matches
    /// `expr`'s shape.
    ///
    /// Used by `run_dispatch` after the §1 single-Identifier short-circuit. The auto-wrap
    /// turns `LET y = z` (today bound to the literal string `"z"`) into a forward-reference
    /// resolving lookup; the ref-name indices feed §8's parking decision.
    pub fn shape_pick(&self, expr: &KExpression<'_>) -> Option<ShapePick> {
        super::dispatcher::shape_pick(self, expr)
    }
}

/// Outcome of a direct-write attempt. `Applied` = success; `Conflict` = `try_borrow_mut`
/// failed and the caller should route through the pending queue. Semantic errors return
/// `Err(KError)` separately; only borrow-conflict cases land in `Conflict`.
enum ApplyOutcome {
    Applied,
    Conflict,
}

/// Helper for `try_apply_function`'s data-insert tail. Inserts `obj` into `data[name]` only
/// if the slot is empty or already a `KFunction`; on collision with a non-function,
/// returns `Rebind`. Removes the matching placeholder on success.
fn apply_function_data_insert<'a>(
    data: &mut std::cell::RefMut<'_, HashMap<String, &'a KObject<'a>>>,
    name: &str,
    obj: &'a KObject<'a>,
    placeholders: &RefCell<HashMap<String, NodeId>>,
) -> Result<ApplyOutcome, KError> {
    if let Some(existing) = data.get(name) {
        if !matches!(existing, KObject::KFunction(_, _)) {
            return Err(KError::new(KErrorKind::Rebind { name: name.to_string() }));
        }
    }
    data.insert(name.to_string(), obj);
    if let Ok(mut ph) = placeholders.try_borrow_mut() {
        ph.remove(name);
    }
    Ok(ApplyOutcome::Applied)
}

/// Exact structural equality for two `ExpressionSignature`s. Same shape (length + per-position
/// keyword/argument tag), same per-Argument `KType`, same return type. Used by
/// `try_apply_function`'s dedupe to reject re-registration of an exact-signature equal but
/// distinct `KFunction`. Doesn't depend on `Argument::name` because two overloads with the
/// same shape and KTypes but different parameter names still collide for dispatch.
fn signatures_exact_equal(
    a: &crate::dispatch::types::ExpressionSignature,
    b: &crate::dispatch::types::ExpressionSignature,
) -> bool {
    use crate::dispatch::types::SignatureElement;
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

/// Output of [`Scope::shape_pick`]. Carries the picked function's per-slot classification:
/// `eager_indices` (Expression parts to schedule as eager sub-Dispatches — today's
/// behavior), `wrap_indices` (bare-Identifier parts in non-literal-name slots that should
/// be auto-wrapped as sub-Dispatches per §7), and `ref_name_indices` (bare-Identifier
/// parts in literal-name slots — `KType::Identifier` or `KType::TypeExprRef` — of a
/// non-pre_run function, used by §8 replay-park).
pub struct ShapePick {
    pub eager_indices: Vec<usize>,
    pub wrap_indices: Vec<usize>,
    pub ref_name_indices: Vec<usize>,
    /// True iff the picked function has `pre_run = Some(_)`. §7 / §8 use this to
    /// distinguish binder-shaped expressions (whose Identifier/TypeExprRef slots are
    /// declarations, not references) from function-call-shaped expressions (whose
    /// literal-name slots are references that may need to park).
    pub picked_has_pre_run: bool,
}

#[cfg(test)]
mod tests {
    use super::{Resolution, RuntimeArena, Scope};
    use crate::dispatch::builtins::test_support::run_root_bare;
    use crate::dispatch::kfunction::{Body, KFunction, NodeId};
    use crate::dispatch::types::{Argument, ExpressionSignature, KType, SignatureElement};
    use crate::dispatch::values::KObject;

    fn unit_signature() -> ExpressionSignature {
        ExpressionSignature {
            return_type: KType::Any,
            elements: vec![SignatureElement::Keyword("FOO".into())],
        }
    }

    fn body_no_op<'a>(
        _scope: &'a Scope<'a>,
        _sched: &mut dyn crate::dispatch::kfunction::SchedulerHandle<'a>,
        _bundle: crate::dispatch::kfunction::ArgumentBundle<'a>,
    ) -> crate::dispatch::kfunction::BodyResult<'a> {
        crate::dispatch::kfunction::BodyResult::Value(
            // Returning a leaked Null here would be safe for tests but the bodies in
            // production paths always return arena allocations; keep parity by allocating
            // through the captured scope.
            _scope.arena.alloc_object(KObject::Null),
        )
    }

    /// Re-entrant `bind_value` while a `data` borrow is held: conditional-defer routes the
    /// write to `pending`; `drain_pending` applies it after the borrow drops. The held
    /// iteration sees the pre-write snapshot (snapshot-iteration semantics), and the
    /// post-drain state has the new entry visible — the foreach-binding pattern.
    #[test]
    fn add_during_active_data_borrow_queues_and_drains() {
        let arena = RuntimeArena::new();
        let scope = run_root_bare(&arena);
        let pre = arena.alloc_object(KObject::Number(1.0));
        scope.bind_value("pre".to_string(), pre).unwrap();

        let new_entry = arena.alloc_object(KObject::Number(2.0));
        {
            // Hold an immutable borrow of `data` (simulates a builtin iterating bindings).
            let snapshot = scope.data.borrow();
            assert!(snapshot.contains_key("pre"));
            // Re-entrant write: queues silently.
            scope.bind_value("during".to_string(), new_entry).unwrap();
            // Iteration sees the pre-write snapshot only.
            assert!(!snapshot.contains_key("during"));
        }
        // Borrow released. Pending still holds the queued write until drain runs.
        assert!(scope.data.borrow().get("during").is_none());
        scope.drain_pending();
        let after = scope.data.borrow();
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
            crate::dispatch::runtime::KErrorKind::Rebind { name } => assert_eq!(name, "x"),
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
        // Cross-scope shadowing is allowed (no Rebind from the child).
        inner.bind_value("x".to_string(), v2).unwrap();
        // Inner sees the inner binding; outer keeps its own.
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
        // A *distinct* KFunction with the exact-equal signature is a DuplicateOverload.
        let f2 = arena.alloc_function(KFunction::new(unit_signature(), Body::Builtin(body_no_op), scope));
        let obj2 = arena.alloc_object(KObject::KFunction(f2, None));
        let err = scope.register_function("FOO".to_string(), f2, obj2).unwrap_err();
        assert!(
            matches!(&err.kind, crate::dispatch::runtime::KErrorKind::DuplicateOverload { name, .. } if name == "FOO"),
            "expected DuplicateOverload, got {err}",
        );
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
        // Different per-slot KType — same untyped shape — no DuplicateOverload.
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
            matches!(&err.kind, crate::dispatch::runtime::KErrorKind::Rebind { name } if name == "FOO"),
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
        // Outer has a Value binding; inner has a Placeholder. Inner.resolve hits the
        // placeholder first and does NOT descend to outer's value.
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
        // Placeholder gone; resolve returns Value.
        assert!(scope.placeholders.borrow().get("x").is_none());
        assert!(matches!(scope.resolve("x"), Resolution::Value(KObject::Number(n)) if *n == 42.0));
    }
}
