use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Write;

use crate::parse::{KExpression, TypeExpr, TypeParams};

use crate::dispatch::kfunction::{ArgumentBundle, KFunction, NodeId};
use crate::dispatch::types::UntypedKey;
use crate::dispatch::values::KObject;
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

/// Lexical environment. `functions` buckets overloads by their *untyped signature* (token
/// shape with slot types erased) so dispatch can pick between same-shape overloads by
/// `KType` specificity. Only the root scope holds a writer in `out`; child scopes have
/// `None` and `write_out` walks `outer` to find one.
///
/// All mutable state is interior-mutable (`RefCell`) so a `&'a Scope<'a>` can be shared
/// across scheduler nodes while builtins still mutate through it.
pub struct Scope<'a> {
    pub outer: Option<&'a Scope<'a>>,
    pub data: RefCell<HashMap<String, &'a KObject<'a>>>,
    pub functions: RefCell<HashMap<UntypedKey, Vec<&'a KFunction<'a>>>>,
    pub out: RefCell<Option<Box<dyn Write + 'a>>>,
    pub arena: &'a RuntimeArena,
    /// Writes that hit a borrow conflict at `bind_value` / `register_function` time.
    /// Drained between dispatch nodes by `drain_pending`; direct writes bypass the queue.
    pending: RefCell<Vec<PendingWrite<'a>>>,
    /// Dispatch-time name placeholders: a binder's `pre_run` hook installs its declared
    /// name here, mapped to the producer slot's `NodeId`, before the binder's body runs.
    /// A consumer looking up the name while the RHS is in flight gets `Placeholder` and
    /// parks on the producer. The binder removes its placeholder before inserting into
    /// `data` / `functions`.
    pub placeholders: RefCell<HashMap<String, NodeId>>,
    /// Lexical-context label set at construction by `child_under_named` (e.g. `"MODULE Foo"`,
    /// `"SIG OrderedSig"`); empty for run-root and ordinary call frames. Record-only;
    /// reserved for future diagnostics.
    pub name: String,
}

impl<'a> Scope<'a> {
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

    pub fn child_for_call(&'a self) -> Scope<'a> {
        Self::child_under(self)
    }

    /// `outer` is the lexical parent — for FN bodies this is the captured definition scope,
    /// not the call site.
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

    /// Like `child_under` but stamps the scope's `name` with a lexical-context label.
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

    /// Bind `name` in this scope. Errors `Rebind` if `data` already holds `name`
    /// (same-scope rebind rejected; cross-scope shadowing allowed). Removes any matching
    /// placeholder this scope owns on success.
    ///
    /// Conditional-defer: direct mutation first, falls back to the `pending` queue iff a
    /// borrow conflict would otherwise panic (caller up the stack iterating `data`).
    pub fn bind_value(&self, name: String, obj: &'a KObject<'a>) -> Result<(), KError> {
        match self.try_apply_value(&name, obj)? {
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

    /// Register `name` as a type-valued binding in this scope. The binding lives in
    /// [`Scope::data`] as a `KObject::TypeExprValue` carrying the bare-leaf surface form
    /// (`TypeExpr { name, params: None }`); the type resolver re-elaborates it to a
    /// [`crate::dispatch::types::KType`] on lookup via
    /// [`crate::dispatch::types::KType::from_type_expr`], which falls back to
    /// [`crate::dispatch::types::KType::from_name`] for the parameterless leaf.
    ///
    /// This is the dual of [`Self::register_function`] for the type half of the binding
    /// surface — the call site that would otherwise reach into `Scope::data` directly to
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
    pub fn register_type(&self, name: String, _ktype: crate::dispatch::types::KType) {
        debug_assert_eq!(
            crate::dispatch::types::KType::from_name(&name),
            Some(_ktype.clone()),
            "register_type({name:?}, {:?}): name does not match KType::from_name",
            _ktype,
        );
        let arena = self.arena;
        let te = TypeExpr { name: name.clone(), params: TypeParams::None };
        let obj: &'a KObject<'a> = arena.alloc_object(KObject::TypeExprValue(te));
        let _ = self.bind_value(name, obj);
    }

    /// Direct-write path for `bind_value`. `Conflict` means borrow contention (caller
    /// queues); `Err` means semantic rejection (not queued).
    ///
    /// When `obj` is a `KObject::KFunction`, the function is *also* mirrored into
    /// `self.functions[signature_key]` so dispatch by the function's signature shape finds
    /// it (supports `LET f = (FN ...)` where the bound name doubles as a callable verb).
    /// Pointer-equality dedupe in the bucket allows intentional LET aliasing
    /// (`LET g = (f)`) without doubling the entry; structural dedupe would over-trigger.
    fn try_apply_value(
        &self,
        name: &str,
        obj: &'a KObject<'a>,
    ) -> Result<ApplyOutcome, KError> {
        // Borrow `functions` before `data` to match `try_apply_function`'s ordering.
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
        if let Ok(mut ph) = self.placeholders.try_borrow_mut() {
            ph.remove(name);
        }
        Ok(ApplyOutcome::Applied)
    }

    /// Direct-write path for `register_function`. Signature-based dedupe rather than
    /// name-based rebind: pointer-equal re-registration is a silent no-op; structurally
    /// exact-equal but distinct is `DuplicateOverload`.
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
        for existing in bucket.iter() {
            if std::ptr::eq(*existing, fn_ref) {
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
                    match self.try_apply_value(&name, obj) {
                        Ok(ApplyOutcome::Applied) => {}
                        Ok(ApplyOutcome::Conflict) => {
                            still_pending.push(PendingWrite::Value { name, obj });
                        }
                        Err(_) => {}
                    }
                }
                PendingWrite::Function { name, fn_ref, obj } => {
                    match self.try_apply_function(&name, fn_ref, obj) {
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

    /// Install a dispatch-time placeholder for `name` -> producer slot `idx`.
    ///
    /// Lenient when `data[name]` already holds a `KObject::KFunction`: silent no-op.
    /// Forward references resolve through the existing function value; a new FN overload
    /// joins the per-signature bucket on finalize without consumers needing to park.
    ///
    /// Errors `Rebind` if `data[name]` holds a non-function or if `placeholders[name]`
    /// already maps to a *different* `NodeId`. Idempotent if re-entered with the same
    /// `NodeId`.
    pub fn install_placeholder(&self, name: String, idx: NodeId) -> Result<(), KError> {
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

    /// Resolve `expr` to a callable, walking `outer` on miss. Ambiguity at the inner scope
    /// does *not* fall through — it surfaces rather than being silently shadowed.
    pub fn dispatch(&self, expr: KExpression<'a>) -> Result<KFuture<'a>, KError> {
        super::dispatcher::dispatch(self, expr)
    }

    pub fn lookup_kfunction(&self, name: &str) -> Option<&'a KFunction<'a>> {
        match self.lookup(name)? {
            KObject::KFunction(f, _) => Some(*f),
            _ => None,
        }
    }

    /// Pick a matching function with at least one `KType::KExpression` slot bound by an
    /// `ExpressionPart::Expression`. Returns the indices of the *eager* `Expression` parts;
    /// lazy ones stay in place for the receiving builtin to dispatch.
    pub fn lazy_candidate(&self, expr: &KExpression<'_>) -> Option<Vec<usize>> {
        super::dispatcher::lazy_candidate(self, expr)
    }

    /// Extended `lazy_candidate` returning eager-Expression, auto-wrap, and forward-ref
    /// index sets for the picked function. `None` if no unique shape match.
    pub fn shape_pick(&self, expr: &KExpression<'_>) -> Option<ShapePick> {
        super::dispatcher::shape_pick(self, expr)
    }
}

/// `Conflict` is reserved for borrow contention; semantic errors come through `Err(KError)`.
enum ApplyOutcome {
    Applied,
    Conflict,
}

/// Insert `obj` into `data[name]` if the slot is empty or already holds a `KFunction`;
/// otherwise `Rebind`. Clears the matching placeholder on success.
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

/// Structural equality on shape + per-Argument `KType` + return type. Independent of
/// `Argument::name` — two overloads with matching shape and types collide for dispatch
/// regardless of parameter naming.
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

/// Per-slot classification for [`Scope::shape_pick`]:
/// - `eager_indices`: `Expression` parts to schedule as eager sub-Dispatches.
/// - `wrap_indices`: bare-Identifier parts in non-literal-name slots to auto-wrap as
///   sub-Dispatches.
/// - `ref_name_indices`: bare-Identifier parts in literal-name slots (`KType::Identifier`
///   / `KType::TypeExprRef`) of a non-`pre_run` function; candidates for replay-park.
///
/// `picked_has_pre_run` distinguishes binder-shaped expressions (literal-name slots are
/// declarations) from call-shaped expressions (literal-name slots are references that may
/// need to park).
pub struct ShapePick {
    pub eager_indices: Vec<usize>,
    pub wrap_indices: Vec<usize>,
    pub ref_name_indices: Vec<usize>,
    pub picked_has_pre_run: bool,
}

#[cfg(test)]
mod tests {
    use super::{Resolution, RuntimeArena, Scope};
    use crate::builtins::test_support::run_root_bare;
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
        crate::dispatch::kfunction::BodyResult::Value(_scope.arena.alloc_object(KObject::Null))
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
            let snapshot = scope.data.borrow();
            assert!(snapshot.contains_key("pre"));
            scope.bind_value("during".to_string(), new_entry).unwrap();
            assert!(!snapshot.contains_key("during"));
        }
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
        assert!(scope.placeholders.borrow().get("x").is_none());
        assert!(matches!(scope.resolve("x"), Resolution::Value(KObject::Number(n)) if *n == 42.0));
    }
}
