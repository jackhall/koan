# Memory model and scoping rules

Every `KObject` lives in a [`RuntimeArena`](../src/dispatch/runtime/arena.rs). Top-level
work allocates into the **run-root arena**; each user-fn call gets its own
**per-call `RuntimeArena`** owned by [`CallArena`](../src/dispatch/runtime/arena.rs),
freed when the call's slot finalizes.

## Scoping: lexical

Free names in a user-fn body resolve through the function's **definition**
scope, carried on [`KFunction.captured`](../src/dispatch/kfunction.rs) — not the
call-site scope. Top-level `FN` definitions capture the run-root, so their free
names resolve through it; nested `FN`s correctly close over their enclosing
locals.

Lexical scoping is what makes the F_{k+1}→F_k chain in tail-recursive code O(1)
memory. Without it, a recursive call would resolve the recursive name through
the call-site scope and pin every prior frame's bindings alive.

## Closure escape: per-call arenas + `Rc`

When a per-call value gets lifted out of its dying frame (typically: a closure
returned from a body, or any value depending on closure-internal state),
[`lift_kobject`](../src/execute/lift.rs) rebuilds it in the destination arena.
Two `KObject` variants carry an optional `Rc<CallArena>` that anchors the
underlying per-call arena alive when needed:

- `KObject::KFunction(&fn, Option<Rc<CallArena>>)` — the closure itself.
  `lift_kobject` compares the lifted function's `captured_scope().arena` pointer
  to the dying frame's arena pointer; match → carry an `Rc` clone, mismatch → no
  `Rc`.
- `KObject::KFuture(KFuture, Option<Rc<CallArena>>)` — `KFuture`s embed
  `&KFunction` plus a bundle and a parsed `KExpression` whose `Future(&KObject)`
  parts can independently point into the dying arena. `lift_kobject` runs a
  targeted membership walk (`kfuture_borrows_dying_arena`) that asks the dying
  arena's `owns_object` side-table whether each embedded `Future(&KObject)`
  borrow points into it, recursing through nested expressions, list/dict
  literals, and bundle arg values; the function reference is checked via the
  same captured-scope-arena equality test the `KFunction` arm uses. Anchor only
  fires when at least one descendant actually borrows into the dying arena.
  `RuntimeArena` records every allocated `KObject`'s stable address (typed-arena
  allocations don't move) in an addresses-only `Vec<usize>` so the membership
  query is a single linear scan with no deref or borrow.

Composite variants (`List`, `Dict`) recurse with a `needs_lift` short-circuit:
when no descendant needs anchoring, the existing `Rc<Vec>`/`Rc<HashMap>` is
cloned in place rather than rebuilt. Koan's collection-immutability contract is
what makes the structural sharing safe.

## Cycle gate on `alloc_object`

The `Rc<CallArena>` escape mechanism creates a self-referential shape if a
composite carrying an escaping closure is re-allocated into the same per-call
arena it already anchors to: the arena's storage holds the composite, the
composite holds the `Rc<CallArena>`, and the `Rc` holds the arena. Neither
side can drop. The case shows up when a body returns a List/Dict/Tagged/Struct
holding a closure — the lift-on-return machinery attaches the per-call frame's
`Rc` to the closure, then a re-allocation of the composite (via `value_pass`,
`Aggregate`, etc.) lands the composite back in the per-call arena.

[`RuntimeArena`](../src/dispatch/runtime/arena.rs) carries an
`escape: Option<*const RuntimeArena>` set by `CallArena::new` to the outer
scope's arena address. `alloc_object` walks the incoming value's composite
tree (`obj_anchors_to`, mirroring `KObject::deep_clone`'s shape) and, on
finding any `Rc<CallArena>` whose `arena()` is `self`, redirects the
allocation up to the escape arena — where the same `Rc` is no longer
self-referential. The redirect is a single `Option`-check on every per-call
`alloc_object`; run-root has `escape: None` and short-circuits, since the
`Rc<CallArena>` shapes the gate looks for can only point at per-call arenas
by construction. The escape pointer is stable for the per-call arena's life
because `CallArena::new` heap-pins the outer arena via `Rc`, and the outer
always outlives this inner per the lexical-scoping invariant.

## Per-call-frame chaining for builtin-built frames

A user-fn call's per-call frame is anchored by lexical scoping: the new frame's
child scope's `outer` is the FN's *captured* scope (run-root for top-level FNs),
which outlives every per-call frame. Builtins that build their own per-call
frame don't always have that property —
[MATCH](../src/dispatch/builtins/match_case.rs) constructs a frame whose child
scope's `outer` is the **call-site** scope, so free names in the branch body
resolve against the surrounding call. When the call site itself lives in a
per-call arena (MATCH inside a user-fn body), the new frame's `outer` pointer
borrows into that arena, and a TCO replace that drops the call-site frame
leaves the new frame with a dangling `outer`.

The fix is a frame-chain Rc on
[`CallArena`](../src/dispatch/runtime/arena.rs): `outer_frame:
Option<Rc<CallArena>>` keeps the parent frame alive whenever the child's
`outer` points into per-call memory. The scheduler exposes the active slot's
frame Rc through
[`SchedulerHandle::current_frame`](../src/dispatch/kfunction.rs), which MATCH
clones into its `CallArena::new` call. `Scheduler::active_frame` is set per
slot run and inherited by `add()` so spawned sub-dispatch / sub-bind /
sub-aggregate slots also see the right ancestor. Top-level FN invokes pass
`None` (their captured chain ends in run-root, which outlives the run, so no
chain is needed and TCO recursion stays bounded).

## Fast path

If a dying arena allocated zero `KFunction`s
([`functions_is_empty`](../src/dispatch/runtime/arena.rs)), no descendant `&KFunction`
can point into it, and `lift_kobject` collapses to a plain `deep_clone`. Owned
variants (`Number`, `KString`, `Bool`, `Null`) `deep_clone` unconditionally —
mildly wasteful for the "value already in dest arena" case, which the design
accepts in exchange for not maintaining full arena-provenance tracking.

## Re-entrant `Scope::add`

[`Scope::add`](../src/dispatch/runtime/scope.rs) tries `try_borrow_mut` on
`data`/`functions` and falls back to a `pending` queue when a borrow is already
held; the scheduler drains the queue between dispatch nodes via
[`drain_pending`](../src/dispatch/runtime/scope.rs). The hot path (no concurrent borrow)
is the same direct insert as before — no measured overhead. Re-entrant writes
that would have panicked now queue silently and become visible after the
iterating borrow releases, with snapshot-iteration semantics for the iterator.

## Structural invariants

Several "must hold" rules are encoded in types rather than checked at runtime:

- `Scope::arena: &'a RuntimeArena` is non-optional; `test_sink()` takes a
  caller-supplied arena.
- `KFunction::captured_scope() -> &'a Scope<'a>` is non-optional.
- The running scope passes through `SchedulerHandle::add_dispatch(expr, scope)`
  directly, so dispatch sites carry their scope explicitly.
- [`RuntimeArena::alloc_function`](../src/dispatch/runtime/arena.rs) `debug_assert`s
  arena-identity between the function and its captured scope, catching a
  misallocated KFunction at the allocation site rather than later as a
  use-after-free in `lift_kobject`'s fast path.

## Performance notes

`finalize_ready_frames` uses a sidecar `frame_holding_slots: Vec<usize>` on
`Scheduler` to find slots needing finalization in O(in-flight calls) rather
than O(scheduler size).

Transient-node reclamation extends the substrate with two more sidecars:
`node_dependencies: Vec<Vec<usize>>` (1:1 with `nodes`, capturing each
Bind/Aggregate slot's owned sub-slot indices at `add()` time before `take()`
consumes the work) and `free_list: Vec<usize>` (LIFO of recyclable indices that
`add()` pulls from before extending). `Scheduler::free` walks `Forward` chain
links and drains the dep sidecar recursively, defensively skipping any
still-live slot. Two trigger points: `run_bind`/`run_aggregate*` free their deps
right after the splice/copy step, and `finalize_ready_frames` chain-frees the
collapsed `Forward(target)` once it has been replaced with the lifted Value.
Reclamation stops at frame holders (their `nodes[i].is_some()` check trips), so
nested user-fn frames each handle their own subtree at their own finalize.

## Verification

- [`repeated_user_fn_calls_do_not_grow_run_root_per_call`](../src/dispatch/builtins/fn_def.rs)
  asserts 50 ECHO calls grow the run-root arena by exactly 50 — one lifted
  return value per call, with all per-call scaffolding freed at call return.
- Closure-escape tests
  ([`closure_escapes_outer_call_and_remains_invocable`](../src/dispatch/builtins/call_by_name.rs),
  [`escaped_closure_with_param_returns_body_value`](../src/dispatch/builtins/call_by_name.rs))
  confirm a closure returned from its defining frame remains invocable.
- [`add_during_active_data_borrow_queues_and_drains`](../src/dispatch/runtime/scope.rs)
  holds a `data` borrow, calls `add`, drops the borrow, drains, and confirms
  the queued write applied — exercising the conditional-defer path.
- [`recursive_tagged_match_no_uaf`](../src/dispatch/builtins/match_case.rs)
  runs a user-fn that recurses through a `Tagged` parameter via MATCH, exercising
  the `outer_frame` chain that keeps the call-site arena alive across TCO replace.
- [`unanchored_kfuture_no_arena_borrow_does_not_anchor`](../src/execute/lift.rs)
  and
  [`unanchored_kfuture_with_arena_borrow_does_anchor`](../src/execute/lift.rs)
  cover both sides of the targeted KFuture anchor: a KFuture whose descendants
  don't borrow into the dying arena lifts with `frame: None`, while one with a
  `Future(&KObject)` allocated in the dying arena anchors with `frame: Some(rc)`.
- [`alloc_object_redirects_self_anchored_value_to_escape_arena`](../src/dispatch/runtime/arena.rs)
  locks in the cycle gate: a value carrying an `Rc<CallArena>` whose `arena()`
  is the receiving arena allocates into the escape arena instead, with the
  per-call arena's storage left untouched.
- The audit slate runs cycle-free: 16 closure-escape, KFuture-anchor, and
  arena-unsafe-site tests plus the cycle-gate regression all pass under
  `MIRIFLAGS=-Zmiri-tree-borrows` with zero UB and zero process-exit leaks,
  signing off the memory model as it stands today.
  [Module-system stage 2](../roadmap/module-system-2-scheduler.md) re-runs
  the slate once the module language and functors flow through the scheduler
  end-to-end, since the new unsafe sites that work introduces reshape the
  runtime that the slate signed off against.

## Open work

- [Module system stage 2 — Module values and functors through the scheduler](../roadmap/module-system-2-scheduler.md)
  — re-run the audit slate against the post-stage-1 runtime plus any new
  unsafe sites that landing modules-and-functors through the scheduler
  introduces.
