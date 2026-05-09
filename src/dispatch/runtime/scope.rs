use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Write;

use crate::parse::kexpression::KExpression;

use crate::dispatch::kfunction::{ArgumentBundle, KFunction};
use crate::dispatch::types::UntypedKey;
use crate::dispatch::values::KObject;
use super::arena::RuntimeArena;
use super::kerror::KError;

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
    /// Lexical-context label for this scope, used only by `debug_path()` for diagnostic
    /// output and by the `KModule` value to remember its source label. Empty string for
    /// run-root and call frames whose context isn't worth naming. Set at construction by
    /// `MODULE`-style builtins (`"MODULE Foo"`, `"SIG OrderedSig"`).
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
            name,
        }
    }

    /// Walk the `outer` chain joining non-empty names with `>`. Used for diagnostics — error
    /// messages can identify the lexical context a dispatch failed in. Run-root and unnamed
    /// frames contribute nothing; the result is empty for an unnamed leaf with no named
    /// ancestor.
    pub fn debug_path(&self) -> String {
        let mut parts: Vec<&str> = Vec::new();
        let mut cur: Option<&Scope<'_>> = Some(self);
        while let Some(s) = cur {
            if !s.name.is_empty() {
                parts.push(&s.name);
            }
            cur = s.outer;
        }
        parts.reverse();
        parts.join(" > ")
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
            let f_ref: &'a KFunction<'a> = f;
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
}

#[cfg(test)]
mod tests {
    use super::{RuntimeArena, Scope};
    use crate::dispatch::values::KObject;

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
}
