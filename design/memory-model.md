# Memory model and scoping rules

Every `KObject` lives in a [`RuntimeArena`](../src/machine/core/arena.rs). Top-level
work allocates into the **run-root arena**; each user-fn call gets its own
**per-call `RuntimeArena`** owned by [`CallArena`](../src/machine/core/arena.rs),
freed when the call's slot finalizes.

## Storage shape: a graph of arena slots

A `RuntimeArena` holds six `typed_arena`-backed sub-arenas — for `KObject`,
`KFunction`, `Scope`, `Module`, `Signature`, and `KType`. Slots have stable
heap addresses; the runtime carries cross-references between them rather
than ownership trees. The structural edges:

- `Scope.outer: Option<&'a Scope<'a>>` — the lexical-parent chain. Many
  sibling scopes can share one outer, so the in-degree is unbounded.
- `Scope.arena: &'a RuntimeArena` — back-pointer to the owning arena.
- [`Bindings.data`](../src/machine/core/bindings.rs) maps each bound name
  to a `&'a KObject<'a>`. The pointee may live in this scope's arena or in
  an outer one.
- [`KFunction.captured`](../src/machine/core/kfunction.rs) holds
  `NonNull<Scope<'a>>` — the closure's definition scope. Multiple
  `KFunction`s share one captured scope when they were defined in the same
  body.
- `KObject::KFunction(&'a KFunction<'a>, Option<Rc<CallArena>>)` and
  `KObject::KFuture(KFuture, Option<Rc<CallArena>>)` carry both a value-side
  reference to a function-arena slot and an optional `Rc<CallArena>` anchor
  to the per-call arena that owns the function's captured scope.
- `Module` and `Signature` cache `*const Scope<'static>` pointers to their
  declaration scopes (heap-pinned by the surrounding arena chain).

**Directionality rule.** References go inward freely — a per-call arena's
slots may point at run-root slots, because the run-root arena outlives every
per-call arena by the lexical-scoping invariant. References that need to
point *outward* — a lifted value referencing a slot in a dying per-call
arena — must carry an `Rc<CallArena>` anchor on the value (or its enclosing
variant) so the per-call arena survives. The lift machinery (see Closure
escape, below) enforces this at the arena boundary.

**Why graph rather than tree.** Many-to-one captures and bindings, sibling
scopes sharing an outer, mutual references between a `Scope` and its
arena's `scopes` sub-arena, and cross-arena `Rc<CallArena>` anchors all
break tree shape. Slots are added incrementally as the program runs;
references can be installed before or after the pointee exists (forward
declarations, replay-park edges). This is the structural backdrop for the
two patterns below — the cycle gate exists because the directionality rule
allows one specific outward cycle, and the frame-chain `Rc` exists because
some builtin-built frames have outer pointers that aren't lexical.

The graph shape is also why the runtime stores `*const T<'static>` and
transmutes on access: a self-referential graph of incrementally added
slots with cross-references doesn't fit the one-owner-builds-one-dependent
shape that self-referential-struct crates model.

## Scoping: lexical

Free names in a user-fn body resolve through the function's **definition**
scope, carried on [`KFunction.captured`](../src/machine/core/kfunction.rs) — not the
call-site scope. Top-level `FN` definitions capture the run-root, so their free
names resolve through it; nested `FN`s correctly close over their enclosing
locals.

Lexical scoping is what makes the F_{k+1}→F_k chain in tail-recursive code O(1)
memory. Without it, a recursive call would resolve the recursive name through
the call-site scope and pin every prior frame's bindings alive.

## Closure escape: per-call arenas + `Rc`

When a per-call value gets lifted out of its dying frame (typically: a closure
returned from a body, or any value depending on closure-internal state),
[`lift_kobject`](../src/machine/execute/lift.rs) rebuilds it in the destination arena.
Three `KObject` shapes carry an optional `Rc<CallArena>` that anchors the
underlying per-call arena alive when needed:

- `KObject::KFunction(&fn, Option<Rc<CallArena>>)` — the closure itself.
  `lift_kobject` compares the lifted function's `captured_scope().arena` pointer
  to the dying frame's arena pointer; match → carry an `Rc` clone, mismatch → no
  `Rc`.
- `KObject::KTypeValue(KType::Module { module, frame })` — a first-class
  module value. The `frame` is the per-call `Rc<CallArena>` of the
  functor call that minted the module (`None` for top-level MODULE
  declarations). `lift_kobject` checks `module.child_scope().arena`
  against the dying frame to decide whether to carry an `Rc`.
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
`Combine`, etc.) lands the composite back in the per-call arena.

[`RuntimeArena`](../src/machine/core/arena.rs) carries an
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
[MATCH](../src/builtins/match_case.rs) constructs a frame whose child
scope's `outer` is the **call-site** scope, so free names in the branch body
resolve against the surrounding call. When the call site itself lives in a
per-call arena (MATCH inside a user-fn body), the new frame's `outer` pointer
borrows into that arena, and a TCO replace that drops the call-site frame
leaves the new frame with a dangling `outer`.

The fix is a frame-chain Rc on
[`CallArena`](../src/machine/core/arena.rs): `outer_frame:
Option<Rc<CallArena>>` keeps the parent frame alive whenever the child's
`outer` points into per-call memory. The scheduler exposes the active slot's
frame Rc through
[`SchedulerHandle::current_frame`](../src/machine/core/kfunction.rs), which MATCH
clones into its `CallArena::new` call. `Scheduler::active_frame` is set per
slot run and inherited by `add()` so spawned sub-dispatch / sub-bind /
sub-combine slots also see the right ancestor. Top-level FN invokes pass
`None` (their captured chain ends in run-root, which outlives the run, so no
chain is needed and TCO recursion stays bounded).

## Tail-step frame reuse

Each TCO step would otherwise drop the previous slot's `CallArena` and
allocate a fresh one — six typed-arena pools, an `Rc<RefCell<Vec<usize>>>`,
an alloc'd child `Scope`, and the `Rc<CallArena>` box itself per iteration.
[`CallArena::try_reset_for_tail`](../src/machine/core/arena.rs) reuses the
shell across iterations: swap the inner `RuntimeArena` for a fresh empty one,
re-allocate the child `Scope` into it, re-link `outer` to the new call's
captured scope. The `Rc`, the heap-pinned arena address, and the slot's
`frame` field carry over unchanged.

Two structural invariants make the reset sound:

- **No escape.** `Rc::get_mut` succeeds iff no other `Rc` to the frame
  exists. Any escaped value (a closure carrying `Some(Rc)`, a list element
  holding one, a sub-Dispatch slot that cloned `active_frame`) keeps
  `strong_count > 1` and refuses the reset, falling through to
  `CallArena::new`. The escape gate's correctness depends on
  [`Scheduler::execute`](../src/machine/execute/scheduler/execute.rs) moving
  `node.frame` into `self.active_frame` (no clone) for the duration of each
  step — so the slot's frame lives in exactly one place when the body runs,
  and any clone visible to `Rc::strong_count` is a real escape.
- **No live external refs into the arena's storage.** By the time TCO
  Replace fires, every sub-Dispatch slot the previous body spawned has
  terminalized and freed, and the slot's `dep_edges` are cleared. The only
  remaining references into the old arena's contents live in the slot's own
  scope, which we're about to rebind. Resetting the storage drops the old
  contents safely.

Frame reuse is what makes deep tail recursion truly constant-memory — both
in the scheduler's slot table (the `Tail` rewrite alone) and on the heap
(the reset turns over arena storage in place rather than allocating per
step). Builtins that build their own frames (MATCH / TRY / EVAL) chain the
call-site frame's `Rc` onto the new frame's `outer_frame`, which keeps
`strong_count > 1` for the call-site frame and routes that iteration through
fresh allocation; cross-step reuse resumes once the builtin's frame is in
turn replaced.

## Fast path

If a dying arena allocated zero `KFunction`s
([`functions_is_empty`](../src/machine/core/arena.rs)), no descendant `&KFunction`
can point into it, and `lift_kobject` collapses to a plain `deep_clone`. Owned
variants (`Number`, `KString`, `Bool`, `Null`) `deep_clone` unconditionally —
mildly wasteful for the "value already in dest arena" case, which the design
accepts in exchange for not maintaining full arena-provenance tracking.

## Re-entrant scope writes

[`Scope::bind_value`](../src/machine/core/scope.rs),
[`Scope::register_function`](../src/machine/core/scope.rs), and
[`Scope::register_type`](../src/machine/core/scope.rs) route through
the embedded [`Bindings`](../src/machine/core/bindings.rs) façade's
validated write primitives (`try_apply` / `try_register_function` /
`try_register_type`), which `try_borrow_mut` the relevant map
(`data` / `functions` / `types`) and return
`ApplyOutcome::Conflict` when a borrow is already held. The scope then defers
the write through the embedded
[`PendingQueue`](../src/machine/core/pending.rs) façade
(`defer_value` / `defer_function` / `defer_type`); the queue is drained by
[`Scope::drain_pending`](../src/machine/core/scope.rs), invoked by the
scheduler between dispatch nodes, which calls `PendingQueue::drain(&Bindings)`
to replay each deferred write through the same validated `Bindings` write path
as a direct insert. The hot path (no concurrent borrow) is one direct insert
with the function-mirror write folded in. Re-entrant writes queue silently and
become visible after the iterating borrow releases, with snapshot-iteration
semantics for the iterator. Drain-time `Err` returns trip a `debug_assert!`
in debug builds (by drain time these are invariant violations); release
builds keep the legacy silent-drop behavior so dispatch nodes never see
surfaced errors.

## Structural invariants

Several "must hold" rules are encoded in types rather than checked at runtime:

- `Scope::arena: &'a RuntimeArena` is non-optional; `test_sink()` takes a
  caller-supplied arena.
- `KFunction::captured_scope() -> &'a Scope<'a>` is non-optional.
- The running scope passes through `SchedulerHandle::add_dispatch(expr, scope)`
  directly, so dispatch sites carry their scope explicitly.
- [`RuntimeArena::alloc_function`](../src/machine/core/arena.rs) `debug_assert`s
  arena-identity between the function and its captured scope, catching a
  misallocated KFunction at the allocation site rather than later as a
  use-after-free in `lift_kobject`'s fast path.

## Performance notes

The push/notify scheduler ([execution-model.md § Push/notify dependency
edges](execution-model.md#pushnotify-dependency-edges)) keeps its slot-table
state in a
[`NodeStore`](../src/machine/execute/scheduler/node_store.rs)
sub-struct that owns `nodes: Vec<Option<Node<'a>>>`, `results:
Vec<Option<NodeOutput<'a>>>`, and `free_list: Vec<usize>` behind the slot
lifecycle `alloc_slot → take_for_run → reinstall* → finalize → free_one`. The
three vectors share an index space; `alloc_slot` is the only path that picks
an index, `finalize` is the only path that lands a terminal `NodeOutput`,
and `free_one` is the only path that clears `results[idx]` and pushes onto
`free_list`. Dependency bookkeeping lives alongside it in a
[`DepGraph`](../src/machine/execute/scheduler/dep_graph.rs) sub-struct
that bundles three `Vec`-shaped fields: `notify_list: Vec<Vec<NodeId>>`
(each producer's dependent list), `pending_deps: Vec<usize>` (each consumer's
unresolved-dep counter), and `dep_edges: Vec<Vec<DepEdge>>` (each slot's
backward edges to producers, tagged `DepEdge::Owned(NodeId)` for sub-slots
the consumer is responsible for reclaiming and `DepEdge::Notify(NodeId)` for
sibling producers the consumer only parked on for wake notification). All
three are 1:1 with `NodeStore`'s slot count; the fields are private and
mutated only through `DepGraph`'s atomic-update methods, so the tri-vector
invariant (every forward edge in `notify_list[p]` matched by a backward
`dep_edges[c]` entry and a +1 in `pending_deps[c]`) is enforced by the
surface rather than by convention.

Transient-node reclamation runs through `Scheduler::reclaim_deps` from
each of the three dep-consuming steps: `run_bind` (after splicing dep
results into `expr.parts` as `ExpressionPart::Future`, *before* resolving
and dispatching the bound expression — so the dispatched body's `add()`
can recycle the freed indices immediately), `run_combine` (after the
finish closure returns), and `run_catch` (after its finish handles the
watched slot's terminal). `reclaim_deps` clears `dep_edges[idx]` and
invokes `Scheduler::free` per dep index; the walk follows `DepGraph::owned_children`,
which only yields `DepEdge::Owned` arms (`Notify` arms are filtered
inside `DepGraph`), so reclaiming a consumer cannot reach a sibling
producer's subtree through a park edge. It skips any still-live slot
via the `NodeStore::is_live` guard, so a free that dives into another
in-flight user-fn call leaves that subtree for that call's own reclamation.

## Verification

- [`repeated_user_fn_calls_do_not_grow_run_root_per_call`](../src/builtins/fn_def.rs)
  asserts 50 ECHO calls grow the run-root arena by exactly 50 — one lifted
  return value per call, with all per-call scaffolding freed at call return.
- Closure-escape tests
  ([`closure_escapes_outer_call_and_remains_invocable`](../src/builtins/call_by_name.rs),
  [`escaped_closure_with_param_returns_body_value`](../src/builtins/call_by_name.rs))
  confirm a closure returned from its defining frame remains invocable.
- [`add_during_active_data_borrow_queues_and_drains`](../src/machine/core/scope.rs)
  holds a `data` borrow, calls `bind_value`, drops the borrow, drains, and
  confirms the queued write applied — exercising the conditional-defer path.
- [`recursive_tagged_match_no_uaf`](../src/builtins/match_case.rs)
  runs a user-fn that recurses through a `Tagged` parameter via MATCH, exercising
  the `outer_frame` chain that keeps the call-site arena alive across TCO replace.
- [`unanchored_kfuture_no_arena_borrow_does_not_anchor`](../src/machine/execute/lift.rs)
  and
  [`unanchored_kfuture_with_arena_borrow_does_anchor`](../src/machine/execute/lift.rs)
  cover both sides of the targeted KFuture anchor: a KFuture whose descendants
  don't borrow into the dying arena lifts with `frame: None`, while one with a
  `Future(&KObject)` allocated in the dying arena anchors with `frame: Some(rc)`.
- [`alloc_object_redirects_self_anchored_value_to_escape_arena`](../src/machine/core/arena.rs)
  locks in the cycle gate: a value carrying an `Rc<CallArena>` whose `arena()`
  is the receiving arena allocates into the escape arena instead, with the
  per-call arena's storage left untouched.
- [`call_arena_try_reset_for_tail_round_trip`](../src/machine/core/arena.rs)
  and
  [`call_arena_try_reset_for_tail_refuses_when_aliased`](../src/machine/core/arena.rs)
  pin the in-place reset: a unique `Rc` resets and re-binds correctly against
  the new outer scope; an aliased `Rc` (the escape case) refuses with the
  frame's arena pointer unchanged.
- [`chained_tail_calls_reuse_frames`](../src/builtins/fn_def.rs)
  asserts that a chain of user-fn tail calls (`AA → BB → CC → DD → PRINT`)
  bumps the scheduler's tail-reuse counter and collapses to one slot.
- The audit slate runs cycle-free across every unsafe site in the runtime
  — closure-escape, KFuture-anchor, arena-unsafe-site, module/signature
  lifetime-erasure transmutes, opaque-ascription re-binds, and type-op
  dispatch through the per-call arena — under
  `MIRIFLAGS=-Zmiri-tree-borrows` with zero UB and zero process-exit
  leaks, signing off the memory model as it stands today. The canonical
  slate list lives in [observe/miri_slate.md](../observe/miri_slate.md).

## Open work

- (none)
