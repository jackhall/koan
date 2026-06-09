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
- [`KFunction.captured`](../src/machine/core/kfunction.rs) holds a
  [`ScopePtr`](../src/machine/core/scope_ptr.rs) — the closure's definition
  scope, lifetime-erased. Multiple `KFunction`s share one captured scope when
  they were defined in the same body.
- `KObject::KFunction(&'a KFunction<'a>, Option<Rc<CallArena>>)` and
  `KObject::KFuture(KFuture, Option<Rc<CallArena>>)` carry both a value-side
  reference to a function-arena slot and an optional `Rc<CallArena>` anchor
  to the per-call arena that owns the function's captured scope.
- `Module` and `Signature` cache their declaration scopes as a
  [`ScopePtr`](../src/machine/core/scope_ptr.rs) (heap-pinned by the surrounding
  arena chain).

**Directionality rule.** References go inward freely — a per-call arena's
slots may point at run-root slots, because the run-root arena outlives every
per-call arena by the lexical-scoping invariant. References that need to
point *outward* — a lifted value referencing a slot in a dying per-call
arena — must carry an `Rc<CallArena>` anchor on the value (or its enclosing
variant) so the per-call arena survives. The lift machinery enforces this at
the arena boundary; see
[per-call-arena-protocol.md § Lift-time anchor decision](per-call-arena-protocol.md#lift-time-anchor-decision).

**Why graph rather than tree.** Many-to-one captures and bindings, sibling
scopes sharing an outer, mutual references between a `Scope` and its
arena's `scopes` sub-arena, and cross-arena `Rc<CallArena>` anchors all
break tree shape. Slots are added incrementally as the program runs;
references can be installed before or after the pointee exists (forward
declarations, replay-park edges). The cycle gate and the frame-chain `Rc`
that ride on top of this graph live in
[per-call-arena-protocol.md](per-call-arena-protocol.md).

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

## Per-call arena protocol

The per-call arena's lifecycle — which `KObject` variants carry an
`Option<Rc<CallArena>>` anchor, how
[`lift_kobject`](../src/machine/execute/lift.rs) decides to attach
one, how the `alloc_object` cycle gate routes self-referential
allocations, how the scheduler propagates the active frame, how
builtin-built frames chain the call-site frame through `outer_frame`,
and how the TCO step reuses the frame shell — is documented in
[per-call-arena-protocol.md](per-call-arena-protocol.md). This file
keeps the storage-shape, scoping, and lifetime-erasure scaffolding the
protocol sits on top of.

## Arena lifetime erasure

Every sub-arena inside [`RuntimeArena`](../src/machine/core/arena.rs) stores
`T<'static>` rather than `T<'a>` — the `'static` is phantom so `RuntimeArena`
itself carries no lifetime parameter. Each named `alloc*` wrapper takes input at
the caller's `'a` and routes one private generic `alloc<K: ArenaStored>` engine:
the engine union-moves the value into its `'static` lifetime family (`At<'static>`)
for storage and re-anchors the returned `&'a` to the input borrow on the way out.
The union move — `Erase<At<'a>, At<'static>>` written then read back through the
other field, with a `const` size assert — is the single erasure all six families
share, so there is one store-side erasure to reason about. It is sound because:

- Lifetimes are zero-sized, so `T<'a>` and `T<'static>` have identical layout.
- `alloc*` returns an `&'a` tied to the input borrow; no `'static` reference
  ever escapes.
- On drop, no stored value's `Drop` impl follows a lifetime-parameterized
  reference — auto-derived `Drop` only touches owned contents. Sub-arenas
  drop together at `RuntimeArena` drop, so any cross-sub-arena `&` is dead
  by the time anyone could observe it.

The scope-pointer case — `CallArena`, `Module`, `Signature`, `KFunction`, and a `Scope`'s
own lexical parent each holding a pointer to a captured, defining, or parent `Scope` — is
centralized in two branded handles in
[`scope_ptr.rs`](../src/machine/core/scope_ptr.rs). The branded
[`ScopePtr<'a>`](../src/machine/core/scope_ptr.rs) backs the carriers that re-hand at an
unbounded `'a`: `erase(&'a Scope<'a>)` records the input's `'a` in the brand, so the carriers
that own a real `'a` — `Module::child_scope` and `Signature::decl_scope` — re-attach
through a **safe** `reattach(&self) -> &'a Scope<'a>`, the brand carrying both the
lifetime bound and (because `Scope<'a>` is invariant) the carrier's invariance in `'a`
structurally. `CallArena` is non-generic — it backs `Rc<CallArena>` and carries no
lifetime — so it stores a `ScopePtr<'static>` and is the `unsafe` re-attach
boundary for that handle: its `scope` / `scope_for_bind` accessors fabricate an `&self`-bounded
lifetime through `reattach_unbounded`, the one transmute the brand cannot supply by safe coercion.
The single irreducible `'static → 'a` cast lives in `scope_ptr.rs`; the carriers no
longer restate it.

The constraint-free twin [`BoundedScopePtr<'a>`](../src/machine/core/scope_ptr.rs) backs the
handles that re-hand *only* behind a reader-bounded borrow: `KFunction::captured` and a
`Scope`'s `outer` lexical parent. Its `get(&'p self) -> &'p Scope<'a>` caps the borrow at the
reader, so the free content `'a` is never cashed unbounded — no borrow==content coupling is
needed, and a frame-bounded child can hold a (possibly frame-bounded) parent without
fabrication. It carries `Scope<'a>`'s invariance structurally for the same reason as `ScopePtr`.

All six families implement the sealed `ArenaStored` trait and route the one gated
[`alloc`](../src/machine/core/arena.rs) engine. `anchors_to` is a required trait
method, so each family declares its cycle behavior at its impl site: `KObject` and
`KType` walk their composite tree for a self-targeting `Rc<CallArena>`, while the
four that cannot hold one — `KFunction`, `Scope`, `Module`, and `Signature` —
declare `anchors_to => false`. The gate is therefore uniform and unbypassable by
construction: a self-anchoring value redirects to the escape arena no matter which
wrapper stored it.

A [`CallArena`](../src/machine/core/arena.rs) bundles a `RuntimeArena`, an
`Option<ScopePtr<'static>>` into it (the child scope; `None` only transiently during
construction and tail-reset), and an `Option<Rc<CallArena>>` for the
parent-frame chain. Two invariants make the ownership unit coherent:

- **Heap-pinning via `Rc`.** `CallArena::new` only ever exposes the frame
  as `Rc<CallArena>`, so the inner arena's heap address is stable for the
  Rc's life and `scope_ptr` (a raw pointer into `arena.scopes`) stays
  valid alongside it. Accessors re-attach lifetimes anchored to `&self`.
- **Field declaration order encodes drop order.** `arena` is declared
  before `outer_frame` so the auto-derived `Drop` tears down this frame's
  arena *before* releasing the parent Rc. Inner pointers die before the
  outer storage they may reference, ruling out a dangling `outer` during
  drop.

A scheduler slot stores a per-call frame scope as a payload-less
[`NodeScope::Yoked`](../src/machine/execute/nodes.rs) marker re-projected from the slot's own
`Node.frame` cart, not a fabricated run-length `&'a Scope<'a>`. The slot-storage scope handle
and the seed-side `with_anchored_child` re-anchor are documented in
[per-call-arena-protocol.md § Slot-table scope handle](per-call-arena-protocol.md#slot-table-scope-handle).

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
Vec<Option<NodeOutput<'a>>>`, `free_list: Vec<usize>`, and
`recent_wakes: Vec<Vec<NodeId>>` (the per-consumer wake-attribution
side-channel scoped to `NodeWork::Dispatch` consumers) behind the slot
lifecycle `alloc_slot → take_for_run → reinstall* → finalize → free_one`. The
slot-indexed vectors share an index space; `alloc_slot` is the only path that
picks an index, `finalize` is the only path that lands a terminal `NodeOutput`,
and `free_one` is the only path that clears `results[idx]`, clears
`recent_wakes[idx]` (retaining the inner Vec's capacity for the next owner —
the side-channel's amortized-allocation pattern), and pushes onto
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
each of the three dep-consuming steps: `resume_eager_subs` (after
splicing dep results into `working_expr.parts` as
`ExpressionPart::Future`, *before* re-resolving and dispatching the
bound expression — so the dispatched body's `add()` can recycle the
freed indices immediately), `run_combine` (after the finish closure
returns), and `run_catch` (after its finish handles the watched slot's
terminal). `reclaim_deps` clears `dep_edges[idx]` and
invokes `Scheduler::free` per dep index; the walk follows `DepGraph::owned_children`,
which only yields `DepEdge::Owned` arms (`Notify` arms are filtered
inside `DepGraph`), so reclaiming a consumer cannot reach a sibling
producer's subtree through a park edge. It skips any still-live slot
via the `NodeStore::is_live` guard, so a free that dives into another
in-flight user-fn call leaves that subtree for that call's own reclamation.

## Verification

- [`add_during_active_data_borrow_queues_and_drains`](../src/machine/core/scope.rs)
  holds a `data` borrow, calls `bind_value`, drops the borrow, drains, and
  confirms the queued write applied — exercising the conditional-defer path.
- Per-call-arena protocol verification (lift anchors, cycle gate, TCO
  frame reuse, MATCH `outer_frame` chain) is enumerated in
  [per-call-arena-protocol.md § Verification](per-call-arena-protocol.md#verification).
- The audit slate runs cycle-free across every unsafe site in the runtime
  under `MIRIFLAGS=-Zmiri-tree-borrows` with zero UB and zero process-exit
  leaks, signing off the memory model as it stands today. The canonical
  slate list lives in [observe/miri_slate.md](../observe/miri_slate.md).
