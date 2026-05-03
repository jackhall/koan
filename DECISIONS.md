# Decisions

Historical record of shipped roadmap items: what was decided, what landed, and the
narrative behind each. Forward-looking work lives in [ROADMAP.md](ROADMAP.md).

Sections are in roughly the order they shipped.

## A builtin for user-defined functions

Surface syntax `FN (<signature>) = (<body>)` where the signature is a `KExpression` mixing
fixed `Keyword` tokens and `Identifier` slots that bind as `Any`-typed `Argument`s; the body
is a `KExpression` evaluated at call time. `KFunction.body` is the enum
[`Body { Builtin(BuiltinFn) | UserDefined(KExpression) }`](src/dispatch/kfunction.rs) — shape
(a) from the original directions, chosen because the upcoming TCO and error-frame work both
wanted to introspect user-fn bodies (a `Box<dyn Fn>` would have hidden them).

Calling convention is parameter substitution: `KFunction::invoke` rewrites parameter
`Identifier`s in a clone of the body to `Future(call-site value)` and returns it as
`BodyResult::Tail` for the scheduler to dispatch in place. Free names (anything not a
parameter) resolve via the call-time scope chain — for top-level `FN` definitions this
coincides with lexical scoping.

Deferred at the time: true lexical closures (a user-fn returning another user-fn that closes
over local state) needed real per-call child scopes, which depended on the leak fix landing
first; substitution doesn't carry the captured scope across function boundaries. (See the
leak-fix section below — closures shipped as part of that work.) Type annotations on
parameter slots remained a future extension hanging off the existing signature parser.
Variadic arguments — the original "function body is a sequence of expressions" sketch —
still want a design pass; the comparator's tiebreak rule for variadic-vs-fixed signatures
is the load-bearing question.

## The dispatch-as-node scheduler refactor

Was not in the original roadmap. The original architecture split dispatch across schedule
time (eager dispatch in `schedule_expr`), execute time (`Pending` nodes), and inline-in-
builtin-bodies (`if_then` and the original `KFunction::invoke` reaching for `scope.dispatch`
directly). Three workarounds for one missing abstraction: only the schedule-time path could
compose with sub-expression evaluation, so user-fn bodies with nested expressions silently
nulled and forward references to user-fns required a try-eager-then-fallback hack.

The refactor made `Dispatch(KExpression)` a scheduler node type. `schedule_expr` collapsed
to "add a `Dispatch` per top-level expression"; the rest is dynamic — `Dispatch` walks its
expression's parts at run time, spawns sub-`Dispatch`/`Bind`/`Aggregate` nodes, and a
builtin body that holds `&mut dyn SchedulerHandle` can also add `Dispatch` nodes (used by
`if_then`'s lazy slot before TCO landed; now superseded by `Tail`). `BuiltinFn`'s return
type became `BodyResult { Value(&KObject) | Tail(KExpression) }`. The `Forward(NodeId)`
mechanism in the result vec lets a body whose result depends on a deferred computation
defer cleanly.

This was the foundational change that made the rest of the user-fn and TCO work tractable.
The next several items design against its shape (the leak fix had to cover scheduler-side
allocations, the error story had to thread errors through the node graph, monadic effects
need access to the same node-spawn lever).

## Tail-call optimization

[`BodyResult::Tail(KExpression)`](src/dispatch/kfunction.rs) makes a builtin's tail return
rewrite the *current scheduler slot's work* to a fresh `Dispatch(expr)` and re-run in place
— no new node allocated, no `Forward` chain. Both deferring builtins (`if_then`,
`KFunction::invoke` for user-fns) were tail by construction and migrated. A chain of tail
calls (`A → B → PRINT` or unbounded `LOOP → LOOP`) reuses one slot end-to-end. Verified
with two slot-count assertions in the test suite.

The roadmap's original concern — host-stack overflow on naïve recursion — was actually
solved earlier by the dispatch-as-node refactor (every "recursive call" enters the FIFO
queue rather than growing the Rust call stack). What `Tail` adds is constant
*scheduler-vec* memory across the tail-call sequence.

What `Tail` did *not* cover at landing time: body-internal sub-expressions — the predicate
of an `IF`-guarded base case, the argument expressions to a recursive call — still allocate
sub-`Dispatch` + `Bind` nodes per iteration, and those nodes are never reclaimed. Realistic
recursive patterns (factorial, list walk) run in O(n) scheduler memory until transient-node
reclamation lands. This is the open "transient-node reclamation" item in the roadmap.

## Replace `Box::leak` with arena-allocated `KObject`s — leak fix

Leak fix shipped via lexical closures + per-call arenas + Rc-counted closure escape.
`Box::leak` is gone from production code (one remaining occurrence in
[scope.rs:326](src/dispatch/scope.rs#L326) is a test-only sentinel marker for the
dispatch-specificity unit tests, not a runtime allocation). Per-call user-fn allocations
(substituted body, child scope, parameter clones, in-body `LET`/`value_pass` allocations)
live in a per-call `RuntimeArena` owned by [`CallArena`](src/dispatch/arena.rs) and freed
when the call's slot finalizes — *unless* a closure that captured the per-call scope has
escaped, in which case the `Rc<CallArena>` carried by the lifted value keeps the arena
alive for as long as the closure is reachable. Free names in user-fn bodies resolve through
the FN's *captured* definition scope ([`KFunction.captured`](src/dispatch/kfunction.rs)) —
lexical scoping for free vars, which broke the F_{k+1}→F_k chain that would otherwise have
made TCO recursion O(n) memory. Top-level FNs capture the run-root, so behavior matches the
old dynamic-scoping model for currently-expressible programs. First-class function values
(returning a fresh FN from a body, binding it via `LET`, calling via `call_by_name`) work
end-to-end.

The leak-fix regression test
[`repeated_user_fn_calls_do_not_grow_run_root_per_call`](src/dispatch/builtins/fn_def.rs)
confirms 50 ECHO calls grow run-root by exactly 50 (one lifted return value per call), down
from the prior 250+.

### Follow-ups that landed alongside the leak fix

These came out of an audit after the leak fix landed and an audit-of-the-audit after the
final stage of work.

1. **`deep_clone` is shallow for reference-bearing `KObject` variants.** Fixed in four
   stages.
   - **Stage 1**: `Box<CallArena>` → `Rc<CallArena>`. The slot's frame Rc drops on
     finalize; the underlying arena drops only when no Rc is held.
   - **Stage 2**: `KObject::KFunction(&fn, Option<Rc<CallArena>>)` — variant gains a frame
     field that keeps the function's underlying per-call arena alive when the closure
     escaped.
   - **Stage 3**: [`lift_kobject`](src/execute/lift.rs) compares the lifted KFunction's
     `captured_scope().arena` pointer to the dying frame's arena pointer; match → carry an
     Rc clone, mismatch → no Rc.
   - **Stage 4 (partial)**: `KObject::KFuture(KFuture, Option<Rc<CallArena>>)` got the same
     treatment — KFutures embed a `&KFunction` plus a bundle and a parsed `KExpression`
     whose `Future(&KObject)` parts can independently point into the dying arena.
     `lift_kobject` anchors any unanchored KFuture descendant conservatively (we don't
     track per-descendant arena provenance, so we attach the dying-frame Rc unconditionally
     on the KFuture arm). KFutures don't escape as values today, so the over-keep is
     theoretical until the planned async work surfaces them.
   - Composite variants (`List`, `Dict`) recurse, with a `needs_lift` short-circuit: when
     no descendant needs anchoring, the existing `Rc<Vec>`/`Rc<HashMap>` is cloned in place
     rather than rebuilt. Koan's collection-immutability contract is what makes the
     structural sharing safe.
   - Tests at [src/dispatch/builtins/call_by_name.rs](src/dispatch/builtins/call_by_name.rs)
     (`closure_escapes_outer_call_and_remains_invocable`,
     `escaped_closure_with_param_returns_body_value`) confirm the previously-UAF
     closure-escape pattern now works.

2. **Lift was unconditional.** [`lift_kobject`](src/execute/lift.rs) has a whole-tree fast
   path: if the dying arena allocated zero `KFunction`s
   ([`functions_is_empty`](src/dispatch/arena.rs#L77)), no descendant `&KFunction` can
   point into it, and the lift collapses to a plain `deep_clone`. For non-fast-path lifts,
   KFunction/KFuture arms attach an Rc only when needed; List/Dict reuse their existing
   `Rc` payload when no descendant needs lifting. Owned variants (Number, KString, Bool,
   Null) still `deep_clone` — that's correct, just mildly wasteful for the "value already
   in dest arena" case, which would need full arena-provenance tracking to eliminate.

3. **`finalize_ready_frames` was O(n²) over scheduler size.** Fixed via a sidecar
   `frame_holding_slots: Vec<usize>` in `Scheduler` updated on stash/finalize.

4. **`Scope::arena: Option<&'a RuntimeArena>` was `None` only for `test_sink()`.** Field
   tightened to `&'a RuntimeArena`; `test_sink()` takes a caller-supplied arena.

5. **`active_scope: Option<*const Scope<'a>>` raw pointer dance.** The running scope is now
   passed through `SchedulerHandle::add_dispatch(expr, scope)` directly; the field and the
   unsafe transmute are gone.

6. **"captured scope must have an arena" — obsolete.**
   [`KFunction::captured_scope()`](src/dispatch/kfunction.rs#L161) returns `&'a Scope<'a>`
   (not `Option`), and `Scope::arena` is `&'a RuntimeArena` (not `Option`). The
   `unwrap_or(scope)` fallback the original audit flagged no longer exists; both shapes are
   non-nullable by construction.

7. **`alloc_function` invariant now `debug_assert`'d.**
   [`RuntimeArena::alloc_function`](src/dispatch/arena.rs#L49) compares `self` against
   `f.captured_scope().arena` and panics in debug builds if a KFunction is being allocated
   into a different arena than its captured scope. Catches the failure mode at the
   allocation site rather than later as a use-after-free in `lift_kobject`'s fast path.
   Verified: all 142 tests pass with the assertion live, confirming current call sites
   (builtin registration, FN definition) hold the invariant.

8. **`Scope::data`/`Scope::functions` re-entrancy footgun resolved via conditional-defer.**
   [`Scope::add`](src/dispatch/scope.rs) now tries `try_borrow_mut` on `data`/`functions`
   and falls back to a `pending` queue when a borrow is already held; the scheduler drains
   the queue between dispatch nodes via [`drain_pending`](src/dispatch/scope.rs). The hot
   path (no concurrent borrow) is the same direct insert as before — no measured overhead.
   Re-entrant writes that would have panicked now queue silently and become visible after
   the iterating borrow releases, with snapshot-iteration semantics for the iterator.
   Regression test [`add_during_active_data_borrow_queues_and_drains`](src/dispatch/scope.rs)
   holds a `data` borrow, calls `add`, drops the borrow, drains, and confirms the queued
   write applied — pre-fix this would have panicked at the second `borrow_mut`.

## Surface dispatch and type errors instead of swallowing as `Null`

`BodyResult` gained an `Err(KError)` arm; the scheduler propagates errors through Forward
chains, short-circuiting any Bind whose dependency errored and appending a `Frame` per
propagation step. [`KError`](src/dispatch/kerror.rs) is a struct (`kind: KErrorKind`,
`frames: Vec<Frame>`) with variants for `TypeMismatch`, `MissingArg`, `UnboundName`,
`ArityMismatch`, `AmbiguousDispatch`, `DispatchFailed`, `ShapeError`, `ParseError`, and
`User` (landing pad for a future RAISE-style builtin). The CLI formats errors to stderr
with the frame chain via [`KError`'s `Display`](src/dispatch/kerror.rs).

The `try_args!` macro grew a default form `try_args!(bundle; arg: Variant, ...)` whose
failure constructs a structured `TypeMismatch` automatically; the original override form
`try_args!(bundle, return $err; ...)` is preserved for the rare site that wants something
other than the default. The `null()` helper now means *intentional* null only — `IF false
THEN x` skipping its lazy slot and `PRINT`'s no-useful-return value are the two surviving
call sites; every other former `null()` site became `err(KError::...)`. `Scope::dispatch`
and `KFunction::bind` returned `Result<KFuture, String>`; both now return
`Result<KFuture, KError>` so dispatch failures (no matching function, ambiguous overload,
arity mismatch in bind) flow through the same channel as builtin errors.
`Scheduler::execute -> Result<(), KError>` and `interpret -> Result<(), KError>` complete
the surfacing.

The user-explicit constraint was no in-language try/catch — errors are values that
propagate implicitly, and "catching" is for builtins to do, not surface syntax. This PR
established the runtime substrate; no catch-builtin shipped with it.

One subtlety: TCO collapses frames. A user-fn whose body tail-calls another user-fn ends
up with only the inner function in the trace, because the slot's `function` field is
replaced at TCO time. Non-tail-call positions (e.g., a sub-Dispatch inside a parens-wrapped
sub-expression) preserve the outer frame via the `frame_holding_slots` finalize path. This
matches how other languages with TCO behave; future work could add per-step frame
accumulation if traces lose too much detail in practice.

## User-defined types and traits — substrate

Function return types are non-optional and enforced at runtime, the parser distinguishes
capitalized type names (`Number`, `KFunction`, `MyType`) from all-caps keywords (`LET`,
`THEN`) and lowercase identifiers, [`KType`](src/dispatch/kfunction.rs) gained
`KFunction`/`List`/`Dict`/`TypeRef` variants so every concrete `KObject` variant has a
name, and [`KType::matches_value`](src/dispatch/kfunction.rs) +
[`KObject::ktype`](src/dispatch/kobject.rs) close the loop on runtime checking. `FN`
surface syntax is `FN (sig) -> ReturnType = (body)` with the type required; the scheduler
injects a check at user-fn slot finalization that surfaces `KErrorKind::TypeMismatch` (with
a `<return>` arg name and a frame naming the called function) on mismatch. Docs in
[README.md](README.md) and [TUTORIAL.md](TUTORIAL.md) were updated.

Known runtime-check limitation: TCO collapses frames, so when A tail-calls B only B's
return type is checked at runtime — the future static pass closes this gap. Builtins are
also not runtime-checked (they return through `BodyResult::Value` with no slot frame);
their declared return types are now honest (LET fixed from `Null` to `Any`,
FN-registration fixed from `Null` to `KFunction`) and the static pass will check them
uniformly.

User-declarable types are still open — see [ROADMAP.md](ROADMAP.md).
