use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Write;

use crate::parse::kexpression::{ExpressionPart, KExpression};

use crate::dispatch::kfunction::{ArgumentBundle, KFunction};
use crate::dispatch::types::{KType, Parseable, SignatureElement, Specificity, UntypedKey};
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
    /// Writes that hit a borrow conflict at `add` time (data/functions already iterated by
    /// some caller up the stack). The scheduler drains this between dispatch nodes via
    /// `drain_pending`. Direct writes (no conflict) bypass the queue entirely, so the hot
    /// path is unchanged. See `add` for the conditional-defer logic.
    pub pending: RefCell<Vec<(String, &'a KObject<'a>)>>,
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
        }
    }

    /// Insert `name → obj` into this scope. Conditional-defer: tries the direct mutation
    /// first, falls back to the `pending` queue iff a borrow conflict would otherwise panic
    /// (typically a caller up the stack is iterating `data` or `functions` and re-entrantly
    /// triggers a write). The scheduler drains the queue between dispatch nodes via
    /// `drain_pending`. The hot path — no concurrent borrow — is the same direct insert as
    /// before; queued writes are observably deferred until drain.
    pub fn add(&self, name: String, obj: &'a KObject<'a>) {
        if let Err(name) = self.try_apply(name, obj) {
            self.pending.borrow_mut().push((name, obj));
        }
    }

    /// Direct-write path. Returns `Ok(())` on success; on borrow conflict returns the unused
    /// `name` back so `add` can move it into the pending queue without cloning. KFunction
    /// dedupe (by pointer) lives here so it applies to both direct and drained writes —
    /// rebinding the same function under a new name must not push a second copy into the
    /// bucket, which would make `pick` report ambiguity. KFunctions are arena-allocated and
    /// never moved, so pointer equality is sufficient.
    fn try_apply(&self, name: String, obj: &'a KObject<'a>) -> Result<(), String> {
        if let KObject::KFunction(f, _) = obj {
            let mut functions = match self.functions.try_borrow_mut() {
                Ok(g) => g,
                Err(_) => return Err(name),
            };
            let mut data = match self.data.try_borrow_mut() {
                Ok(d) => d,
                Err(_) => return Err(name),
            };
            let key = f.signature.untyped_key();
            let f_ref: &'a KFunction<'a> = *f;
            let bucket = functions.entry(key).or_default();
            if !bucket.iter().any(|existing| std::ptr::eq(*existing, f_ref)) {
                bucket.push(f_ref);
            }
            data.insert(name, obj);
        } else {
            let mut data = match self.data.try_borrow_mut() {
                Ok(d) => d,
                Err(_) => return Err(name),
            };
            data.insert(name, obj);
        }
        Ok(())
    }

    /// Apply queued writes. Called by the scheduler after each dispatch node so writes that
    /// queued during a re-entrant borrow become visible by the next node's run. Items that
    /// still hit a borrow conflict (rare — would mean drain itself was re-entered through a
    /// live borrow) stay queued for the next drain attempt; the queue is therefore eventually
    /// consistent rather than guaranteed-empty after one call.
    pub fn drain_pending(&self) {
        if self.pending.borrow().is_empty() {
            return;
        }
        let pending = std::mem::take(&mut *self.pending.borrow_mut());
        let mut still_pending: Vec<(String, &'a KObject<'a>)> = Vec::new();
        for (name, obj) in pending {
            if let Err(name) = self.try_apply(name, obj) {
                still_pending.push((name, obj));
            }
        }
        if !still_pending.is_empty() {
            self.pending.borrow_mut().extend(still_pending);
        }
    }

    /// Look up `name` in this scope, walking the `outer` chain on miss. Returns the bound
    /// `KObject` from the nearest enclosing scope, or `None` if unbound at every level.
    pub fn lookup(&self, name: &str) -> Option<&'a KObject<'a>> {
        if let Some(obj) = self.data.borrow().get(name).copied() {
            return Some(obj);
        }
        self.outer.and_then(|outer| outer.lookup(name))
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
        match self.pick(&expr) {
            Pick::One(f) => return f.bind(expr),
            Pick::Ambiguous(n) => {
                return Err(KError::new(KErrorKind::AmbiguousDispatch {
                    expr: expr.summarize(),
                    candidates: n,
                }));
            }
            Pick::None => {}
        }
        if let Some(outer) = self.outer {
            return outer.dispatch(expr);
        }
        Err(KError::new(KErrorKind::DispatchFailed {
            expr: expr.summarize(),
            reason: "no matching function".to_string(),
        }))
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
        if !expr.parts.iter().any(|p| matches!(p, ExpressionPart::Expression(_))) {
            return None;
        }
        let functions = self.functions.borrow();
        let mut viable: Vec<(&KFunction<'a>, Vec<usize>)> = functions
            .get(&expr.untyped_key())
            .into_iter()
            .flatten()
            .filter_map(|f| lazy_eager_indices(f, expr).map(|e| (*f, e)))
            .collect();
        if !viable.is_empty() {
            let funcs: Vec<&KFunction<'_>> = viable.iter().map(|(f, _)| *f).collect();
            // Ambiguous → return None and let `dispatch` surface the actual error at execute
            // time. Falling back to the eager pipeline here would misevaluate the lazy slot.
            return pick_most_specific_index(&funcs).map(|i| viable.swap_remove(i).1);
        }
        drop(functions);
        self.outer.and_then(|outer| outer.lazy_candidate(expr))
    }

    /// Internal: pick within this scope's own bucket only. Returns `None` if the bucket is
    /// missing or has no matching candidates; the caller decides whether to walk `outer`.
    fn pick(&self, expr: &KExpression<'a>) -> Pick<'a> {
        let key = expr.untyped_key();
        let functions = self.functions.borrow();
        let bucket = match functions.get(&key) {
            Some(b) => b,
            None => return Pick::None,
        };
        let candidates: Vec<&'a KFunction<'a>> = bucket
            .iter()
            .filter(|f| f.signature.matches(expr))
            .copied()
            .collect();
        match pick_most_specific_index(&candidates) {
            Some(i) => Pick::One(candidates[i]),
            None if candidates.is_empty() => Pick::None,
            None => Pick::Ambiguous(candidates.len()),
        }
    }
}

enum Pick<'a> {
    One(&'a KFunction<'a>),
    Ambiguous(usize),
    None,
}

/// Pairwise specificity tournament: returns `Some(i)` iff `candidates[i]` is strictly more
/// specific than every other candidate. Returns `None` if the bucket is empty or if no
/// candidate dominates every peer (callers distinguish via `candidates.is_empty()`).
fn pick_most_specific_index(candidates: &[&KFunction<'_>]) -> Option<usize> {
    candidates
        .iter()
        .enumerate()
        .find(|(i, a)| {
            candidates.iter().enumerate().all(|(j, b)| {
                *i == j
                    || matches!(a.signature.specificity_vs(&b.signature), Specificity::StrictlyMore)
            })
        })
        .map(|(i, _)| i)
}

/// `lazy_candidate` shape check for a single function: is this a viable lazy match for `expr`,
/// and if so what are the indices of its eager-Expression parts? Returns `None` when the
/// function isn't a lazy candidate (length mismatch, fixed-token mismatch, no `KExpression`
/// slot binding an `Expression` part, or any other arg-type mismatch).
fn lazy_eager_indices(f: &KFunction<'_>, expr: &KExpression<'_>) -> Option<Vec<usize>> {
    let sig = &f.signature;
    if sig.elements.len() != expr.parts.len() {
        return None;
    }
    let mut eager_indices: Vec<usize> = Vec::new();
    let mut has_lazy_slot = false;
    for (i, (el, part)) in sig.elements.iter().zip(expr.parts.iter()).enumerate() {
        match (el, part) {
            (SignatureElement::Keyword(s), ExpressionPart::Keyword(t)) if s == t => {}
            (SignatureElement::Keyword(_), _) => return None,
            (SignatureElement::Argument(arg), part) => match (&arg.ktype, part) {
                (KType::KExpression, ExpressionPart::Expression(_)) => {
                    has_lazy_slot = true;
                }
                (KType::KExpression, _) => return None,
                (_, ExpressionPart::Expression(_)) => {
                    // Speculative: assume the eager-evaluated result will type-match at late
                    // dispatch. If not, dispatch will fail at that point.
                    eager_indices.push(i);
                }
                (_, other) => {
                    if !arg.matches(other) {
                        return None;
                    }
                }
            },
        }
    }
    if has_lazy_slot { Some(eager_indices) } else { None }
}

#[cfg(test)]
mod tests {
    use super::{RuntimeArena, Scope};
    use crate::dispatch::builtins::default_scope;
    use crate::parse::kexpression::{ExpressionPart, KExpression, KLiteral};

    #[test]
    fn dispatch_walks_outer_chain_to_find_builtin() {
        // Parent owns the LET builtin; child has no functions of its own. Dispatching LET
        // against the child must climb to the parent.
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

        assert!(inner.dispatch(expr).is_ok(), "child scope should inherit LET from outer");
    }

    #[test]
    fn dispatch_with_no_outer_and_no_match_errors() {
        let arena = RuntimeArena::new();
        let scope = arena.alloc_scope(Scope::run_root(&arena, None, Box::new(std::io::sink())));
        let expr = KExpression {
            parts: vec![ExpressionPart::Identifier("nope".into())],
        };
        assert!(scope.dispatch(expr).is_err());
    }

    /// Re-entrant `add` while a `data` borrow is held: conditional-defer routes the write
    /// to `pending`; `drain_pending` applies it after the borrow drops. The held iteration
    /// sees the pre-write snapshot (snapshot-iteration semantics), and the post-drain
    /// state has the new entry visible — the foreach-binding pattern.
    #[test]
    fn add_during_active_data_borrow_queues_and_drains() {
        let arena = RuntimeArena::new();
        let scope = arena.alloc_scope(Scope::run_root(&arena, None, Box::new(std::io::sink())));
        let pre = arena.alloc_object(KObject::Number(1.0));
        scope.add("pre".to_string(), pre);

        let new_entry = arena.alloc_object(KObject::Number(2.0));
        {
            // Hold an immutable borrow of `data` (simulates a builtin iterating bindings).
            let snapshot = scope.data.borrow();
            assert!(snapshot.contains_key("pre"));
            // Re-entrant write: queues silently.
            scope.add("during".to_string(), new_entry);
            // Iteration sees the pre-write snapshot only.
            assert!(!snapshot.contains_key("during"));
        }
        // Borrow released. Pending still holds the queued write until drain runs.
        assert!(scope.data.borrow().get("during").is_none());
        scope.drain_pending();
        let after = scope.data.borrow();
        assert!(matches!(after.get("during"), Some(KObject::Number(n)) if *n == 2.0));
    }

    // --- specificity / bucketing / shadowing tests for the dispatch refactor ---

    use crate::dispatch::builtins::register_builtin;
    use crate::dispatch::kfunction::{ArgumentBundle, BodyResult, SchedulerHandle};
    use crate::dispatch::types::{Argument, ExpressionSignature, KType, SignatureElement};
    use crate::dispatch::values::KObject;
    use crate::execute::scheduler::Scheduler;

    // Sentinel-returning bodies. Each produces a distinct `KString` so a test can tell which
    // overload won dispatch. The explicit `'a` is needed so the leaked `&'static KObject<'static>`
    // marker coerces (covariantly) to `&'a KObject<'a>`.
    fn marker<'a>(s: &'static str) -> &'a KObject<'a> {
        Box::leak(Box::new(KObject::KString(s.into())))
    }

    fn body_identifier<'a>(_s: &'a Scope<'a>, _h: &mut dyn SchedulerHandle<'a>, _a: ArgumentBundle<'a>) -> BodyResult<'a> { BodyResult::Value(marker("identifier")) }
    fn body_any<'a>(_s: &'a Scope<'a>, _h: &mut dyn SchedulerHandle<'a>, _a: ArgumentBundle<'a>) -> BodyResult<'a> { BodyResult::Value(marker("any")) }
    fn body_number_any<'a>(_s: &'a Scope<'a>, _h: &mut dyn SchedulerHandle<'a>, _a: ArgumentBundle<'a>) -> BodyResult<'a> { BodyResult::Value(marker("number_any")) }
    fn body_any_number<'a>(_s: &'a Scope<'a>, _h: &mut dyn SchedulerHandle<'a>, _a: ArgumentBundle<'a>) -> BodyResult<'a> { BodyResult::Value(marker("any_number")) }
    fn body_inner_any<'a>(_s: &'a Scope<'a>, _h: &mut dyn SchedulerHandle<'a>, _a: ArgumentBundle<'a>) -> BodyResult<'a> { BodyResult::Value(marker("inner_any")) }
    fn body_outer_number<'a>(_s: &'a Scope<'a>, _h: &mut dyn SchedulerHandle<'a>, _a: ArgumentBundle<'a>) -> BodyResult<'a> { BodyResult::Value(marker("outer_number")) }
    fn body_lowercase<'a>(_s: &'a Scope<'a>, _h: &mut dyn SchedulerHandle<'a>, _a: ArgumentBundle<'a>) -> BodyResult<'a> { BodyResult::Value(marker("lowercase")) }

    fn one_slot_sig(name: &str, kt: KType) -> ExpressionSignature {
        ExpressionSignature {
            return_type: KType::Any,
            elements: vec![SignatureElement::Argument(Argument {
                name: name.into(),
                ktype: kt,

            })],
        }
    }

    /// `<a:A> OP <b:B>` — a binary-operator shape that includes a fixed token so the
    /// expression doesn't get caught by list-shape detection (which would treat any
    /// fixed-token-free multi-part expression as a list construction).
    fn two_slot_sig(a: KType, b: KType) -> ExpressionSignature {
        ExpressionSignature {
            return_type: KType::Any,
            elements: vec![
                SignatureElement::Argument(Argument {
                    name: "a".into(),
                    ktype: a,

                }),
                SignatureElement::Keyword("OP".into()),
                SignatureElement::Argument(Argument {
                    name: "b".into(),
                    ktype: b,

                }),
            ],
        }
    }

    /// Register the `Identifier` overload AFTER the `Any` overload (the opposite of
    /// `default_scope`'s declaration order). Specificity-based dispatch should still pick
    /// `Identifier` for an identifier-shaped input.
    #[test]
    fn dispatch_picks_identifier_over_any_regardless_of_registration_order() {
        let arena = RuntimeArena::new();
        let scope = arena.alloc_scope(Scope::run_root(&arena, None, Box::new(std::io::sink())));
        register_builtin(scope, "any_first", one_slot_sig("v", KType::Any), body_any);
        register_builtin(scope, "ident_second", one_slot_sig("v", KType::Identifier), body_identifier);

        let expr = KExpression { parts: vec![ExpressionPart::Identifier("foo".into())] };
        let mut sched = Scheduler::new();
        let id = sched.add_dispatch(expr, scope);
        sched.execute().unwrap();
        let result = sched.read(id);
        assert!(matches!(result, KObject::KString(s) if s == "identifier"),
            "Identifier overload should win on an identifier input, got {:?}", summarize_marker(result));
    }

    /// Inner scope's `Any` overload shadows the outer scope's more-specific `Number` overload.
    /// Pure lexical shadowing — innermost match wins regardless of specificity at outer levels.
    #[test]
    fn dispatch_inner_scope_shadows_outer_more_specific() {
        let arena = RuntimeArena::new();
        let outer = arena.alloc_scope(Scope::run_root(&arena, None, Box::new(std::io::sink())));
        register_builtin(outer, "outer_specific", one_slot_sig("v", KType::Number), body_outer_number);

        let inner = arena.alloc_scope(outer.child_for_call());
        register_builtin(inner, "inner_loose", one_slot_sig("v", KType::Any), body_inner_any);

        let expr = KExpression { parts: vec![ExpressionPart::Literal(KLiteral::Number(7.0))] };
        let mut sched = Scheduler::new();
        let id = sched.add_dispatch(expr, inner);
        sched.execute().unwrap();
        let result = sched.read(id);
        assert!(matches!(result, KObject::KString(s) if s == "inner_any"),
            "inner Any must shadow outer Number (lexical shadowing > specificity), got {:?}",
            summarize_marker(result));
    }

    /// `<Number> OP <Any>` and `<Any> OP <Number>` are incomparable for an input matching
    /// both (`5 OP 7`): each is more specific in one slot and less in the other. Dispatch
    /// must error rather than silently picking one. The fixed `OP` token keeps the
    /// expression out of the list-shape short-circuit.
    #[test]
    fn dispatch_errors_on_ambiguous_overlap() {
        let arena = RuntimeArena::new();
        let scope = arena.alloc_scope(Scope::run_root(&arena, None, Box::new(std::io::sink())));
        register_builtin(scope, "number_any", two_slot_sig(KType::Number, KType::Any), body_number_any);
        register_builtin(scope, "any_number", two_slot_sig(KType::Any, KType::Number), body_any_number);

        let expr = KExpression {
            parts: vec![
                ExpressionPart::Literal(KLiteral::Number(5.0)),
                ExpressionPart::Keyword("OP".into()),
                ExpressionPart::Literal(KLiteral::Number(7.0)),
            ],
        };
        let result = scope.dispatch(expr);
        match result {
            Err(e) => assert!(
                matches!(e.kind, crate::dispatch::runtime::KErrorKind::AmbiguousDispatch { .. }),
                "expected ambiguity error, got: {e}",
            ),
            Ok(_) => panic!("equally-specific overloads should produce an ambiguity error"),
        }
    }

    /// A lowercase fixed token in a registered signature is coerced to uppercase, so
    /// dispatching the uppercase form from a source program still hits the registered
    /// function. (Once monadic effects exist, this should also produce a warning effect.)
    #[test]
    fn registration_coerces_lowercase_fixed_tokens_to_uppercase() {
        let arena = RuntimeArena::new();
        let scope = arena.alloc_scope(Scope::run_root(&arena, None, Box::new(std::io::sink())));
        let sig = ExpressionSignature {
            return_type: KType::Any,
            elements: vec![
                SignatureElement::Keyword("foo".into()), // lowercase — should be coerced
                SignatureElement::Argument(Argument {
                    name: "v".into(),
                    ktype: KType::Number,

                }),
            ],
        };
        register_builtin(scope, "FOO", sig, body_lowercase);

        // The source-side caller writes `FOO 1` (uppercase), which must match the coerced
        // `FOO <v>` registration.
        let expr = KExpression {
            parts: vec![
                ExpressionPart::Keyword("FOO".into()),
                ExpressionPart::Literal(KLiteral::Number(1.0)),
            ],
        };
        let mut sched = Scheduler::new();
        let id = sched.add_dispatch(expr, scope);
        sched.execute().unwrap();
        let result = sched.read(id);
        assert!(matches!(result, KObject::KString(s) if s == "lowercase"));
    }

    fn summarize_marker(obj: &KObject<'_>) -> String {
        match obj {
            KObject::KString(s) => s.clone(),
            KObject::Null => "null".into(),
            _ => "<other>".into(),
        }
    }
}
