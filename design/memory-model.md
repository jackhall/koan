# Memory model and scoping rules

Every `KObject` lives in a [`RuntimeArena`](../src/machine/core/arena.rs). Top-level
work allocates into the **run-root arena**; each user-fn call gets its own
**per-call `RuntimeArena`** owned by [`CallArena`](../src/machine/core/arena.rs),
freed when the call's slot finalizes.

## Storage shape: a graph of arena slots

A `RuntimeArena` holds seven `typed_arena`-backed sub-arenas — for `KObject`,
`KFunction`, `Scope`, `Module`, `Signature`, `KType`, and `OperatorGroup`. Slots have stable
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
- `KObject::KFunction(&'a KFunction<'a>, Option<Rc<FrameStorage>>)` and
  `KObject::KFuture(KFuture, Option<Rc<FrameStorage>>)` carry both a value-side
  reference to a function-arena slot and an optional `Rc<FrameStorage>` anchor
  to the per-call arena that owns the function's captured scope.
- `Module` and `Signature` cache their declaration scopes as a
  [`ScopePtr`](../src/machine/core/scope_ptr.rs) (heap-pinned by the surrounding
  arena chain).

**Directionality rule.** References go inward freely — a per-call arena's
slots may point at run-root slots, because the run-root arena outlives every
per-call arena by the lexical-scoping invariant. References that need to
point *outward* — a lifted value referencing a slot in a dying per-call
arena — must carry an `Rc<FrameStorage>` anchor on the value (or its enclosing
variant) so the per-call arena survives. The lift machinery enforces this at
the arena boundary; see
[per-call-arena-protocol.md § Lift-time anchor decision](per-call-arena-protocol.md#lift-time-anchor-decision).

**Why graph rather than tree.** Many-to-one captures and bindings, sibling
scopes sharing an outer, mutual references between a `Scope` and its
arena's `scopes` sub-arena, and cross-arena `Rc<FrameStorage>` anchors all
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
`Option<Rc<FrameStorage>>` anchor, how
[`lift_kobject`](../src/machine/execute/lift.rs) decides to attach
one, how the `alloc_object` cycle gate routes self-referential
allocations, how the scheduler propagates the active frame, how
builtin-built frames chain the call-site frame's storage through
`FrameStorage.outer`, and how the TCO step reuses the frame shell over a
fresh `FrameStorage` — is documented in
[per-call-arena-protocol.md](per-call-arena-protocol.md). This file
keeps the storage-shape, scoping, and lifetime-erasure scaffolding the
protocol sits on top of.

## Arena lifetime erasure

Every sub-arena inside [`RuntimeArena`](../src/machine/core/arena.rs) stores
`T<'static>` rather than `T<'a>` — the `'static` is phantom so `RuntimeArena`
itself carries no lifetime parameter. The erase-store engine lives generically in
the [`StorageFrame<W>`](../src/machine/core/storage_frame.rs) substrate (`RuntimeArena`
is the Koan instantiation `StorageFrame<KoanStorageProfile>`). Each named `alloc*` wrapper
takes input at the caller's `'a` and routes one `alloc<K: Stored>` engine: the engine
union-moves the value into its `'static` lifetime family (`At<'static>`) for storage and
re-anchors the returned `&'a` to the input borrow on the way out. The union move —
`Erase<At<'a>, At<'static>>` written then read back through the other field, with a
`const` size assert — is the single erasure every family shares, so there is one
store-side erasure to reason about. It is sound because:

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
structurally.

Two lifetime-free carriers store a `ScopePtr<'static>` and fabricate the content lifetime back,
because neither can brand it. `CallArena` is non-generic — it backs `Rc<CallArena>` and carries no
lifetime — so its `scope` / `scope_for_bind` accessors fabricate an `&self`-bounded lifetime through
the `unsafe` `reattach_unbounded`. A scheduler slot's `NodeScope::Anchored` evicts a genuinely
run-lived scope off the lifetime-free node and re-attaches a free content lifetime behind an
`&self`-bounded borrow through the `unsafe` `reattach_bounded` (sound because the pointee lives for
all of `'run`). Both reach the `'static` store through `ScopePtr::erase_static`, a brand-dropping
constructor that is *safe* to call (forgetting a lifetime cannot fabricate one); the fabrication
hazard is deferred to the `unsafe` re-attach, witnessed by the carrier's pinning (the frame `Rc`) or,
for an `Anchored` node, the scope being run-lived. The irreducible `'static → 'a` casts live in
`scope_ptr.rs`; the carriers no longer restate them.

The constraint-free twin [`BoundedScopePtr<'a>`](../src/machine/core/scope_ptr.rs) backs the
handles that re-hand *only* behind a reader-bounded borrow: `KFunction::captured` and a
`Scope`'s `outer` lexical parent. Its `get(&'p self) -> &'p Scope<'a>` caps the borrow at the
reader, so the free content `'a` is never cashed unbounded — no borrow==content coupling is
needed, and a frame-bounded child can hold a (possibly frame-bounded) parent without
fabrication. It carries `Scope<'a>`'s invariance structurally for the same reason as `ScopePtr`.

Beyond the store-side erasure and the branded scope pointers, a handful of carriers store a
borrow-carrying *value* on a structure the borrow checker cannot lifetime-track — a scheduler
node's slot, a per-call `TraceFrame` — and re-anchor it at a caller-chosen lifetime on read,
witnessed by a held `Rc`. Moving a value along a dep edge is the scheduler's job, so the
erase/reattach discipline that makes the move safe lives in the scheduler:
[`scheduler/erase.rs`](../src/scheduler/erase.rs) declares `unsafe trait Reattachable { type At<'r>; }` —
a family whose representation is identical across every choice of its single lifetime — and
[`Erased<T>`](../src/scheduler/erase.rs) stores that family's `At<'static>` form. A single
private `retype<A, B>` — a `transmute_copy` through a `ManuallyDrop` (plain `transmute` cannot prove
two opaque GAT projections share a size), guarded by a `const` size assert that restores the check
`transmute` would emit, mirroring the sibling `erase_store` — is the only place a
`T::At<'a> → T::At<'b>` lifetime retype is written; `Erased::erase` / `Erased::reattach` and the
transient `reattach_value` / `reattach_ref` / `reattach_slice` helpers all route it. The carrier families live beside their own
types as declarative `unsafe impl Reattachable` instantiations — `ContractFamily` for the
node's [`ErasedContract`](../src/machine/core/kfunction/body.rs), `CarriedFamily` / `ContFamily` for
the scheduler value (`Workload::Value`) and continuation (`ErasedCont`),
`ResultCarriedFamily` for the transient step-lifetime re-anchor (`deps_at_step`) in
`outcome.rs`, and `ScopeFamily` so the branded `ScopePtr` re-attaches and the arena's
`&Scope → &Scope<'static>` storage erasures route the same primitive — so `erase.rs` names no
concrete Koan type and the scheduler stays workload-independent (the workload depends on the
scheduler for the machinery, not the reverse). The liveness witness is not a
parameter on `reattach`: each call site holds the pinning `Rc` (the frame cart, the run arena)
across the re-anchored read, and the per-carrier doc names which one.

The value channel itself is borrow-checked end to end: the scheduler stores a finalized terminal as
`Erased<W::Value>` ([`node_store.rs`](../src/scheduler/node_store.rs)), erasing it inside `finalize`,
and a read (`read_result` / `read` / `read_result_with_frame`) re-anchors it to the read's own
`&self` borrow — `Live<'node, W>`. Because `free_one` / `finalize` need `&mut self`, the co-stored
producer-frame `Rc` cannot drop while a read borrow is live, so the re-anchored `'node` lifetime
cannot outlive the backing arena: the pin-outlives-read fact is a borrow the compiler checks rather
than a SAFETY comment the driver asserts. The driver's transient reads
([`KoanRuntime::read_result`](../src/machine/execute/runtime.rs), the
[`SchedulerView`](../src/machine/execute/dispatch/ctx.rs) forwarder) consume that `'node` value with
no `unsafe` of their own. The consumer-pull lift and the Done contract hook re-anchor their reads at
a *node* lifetime, not a fabricated `'run`: `read_lifted` lifts each dep (and the `Outcome::Forward`
ready pull) into the consumer scope's arena bounded by the active cart `Rc`, and a Done terminal is
finalized at its step lifetime `'step` *within* the step that produced it (the run loop's `run_step`
erases it into the slot store before the step's frame witness drops). `pin_carried_to_run` survives
for one genuine `'run` re-home only — the consumer-less root drain in
[`run_program`](../src/machine/execute/runtime/interpret.rs), which lifts each top-level terminal
into the run-global root arena.

A sibling primitive in [`reattach.rs`](../src/machine/core/reattach.rs), `pin_deref`, owns the
*other* unsafe shape — re-borrowing a raw `*const T` whose pointee a heap pin holds fixed (the
`Rc<FrameStorage>`-pinned arena pointer, the storage engine's escape frame). Erase/reattach
moves a value between lifetimes; `pin_deref` recovers a reference from a pointer the borrow checker
never tracked, so it stays in `machine::core` (it recovers a pointer an arena pins, not a value
moving between nodes) as the one audited home for the `&*ptr` the arena and storage engine would
otherwise each open inline. The
store side carries no `unsafe` at all: `ScopePtr::erase` builds its stored pointer with the safe
`NonNull::from(scope).cast()`, deferring every fabrication hazard to the re-attach.

Every family implements the `Stored` trait and routes the one gated
[`alloc`](../src/machine/core/storage_frame.rs) engine. `anchors_to` is a required trait
method, so each family declares its cycle behavior at its impl site: `KObject` and
`KType` walk their composite tree for a self-targeting `Rc<FrameStorage>`, while the
families that cannot hold one — `KFunction`, `Scope`, `Module`, `Signature`, and
`OperatorGroup` — declare `anchors_to => false`. The gate is therefore uniform and
unbypassable by construction: `Stored` is unsealed (an in-crate extension point), but
the substrate's `storage` bundle is private and `alloc` is the only path to it, so no
impl can route a value around the redirect. A self-anchoring value redirects to the
escape arena no matter which wrapper stored it.

A [`CallArena`](../src/machine/core/arena.rs) is a thin shell over a refcounted
[`FrameStorage`](../src/machine/core/arena.rs): the shell carries a `Rc<FrameStorage>` and an
`Option<ScopePtr<'static>>` (the child scope; `None` only transiently during construction), while
`FrameStorage` bundles the `RuntimeArena` and an `Option<Rc<FrameStorage>>` for the parent-frame
chain. The shell/storage split lets an escaping value pin only the storage, leaving the shell
uniquely owned for tail reuse (see
[per-call-arena-protocol.md § TCO frame reuse](per-call-arena-protocol.md#tco-frame-reuse)). Two
invariants make the ownership unit coherent:

- **Heap-pinning via `Rc`.** `CallArena::new` builds the arena inside its own
  `Rc<FrameStorage>` and only ever exposes the frame as `Rc<CallArena>`, so the inner
  arena's heap address is stable for the storage Rc's life and `scope_ptr` (a raw
  pointer into `arena.scopes`) stays valid alongside it. Accessors re-attach lifetimes
  anchored to `&self`. A tail reset installs a *fresh* `FrameStorage`, so the arena
  address changes across a reset — no accessor captures it across one, and the borrow
  checker forbids safe code from doing so.
- **Field declaration order encodes drop order.** On `FrameStorage`, `arena` is declared
  before `outer` so the auto-derived `Drop` tears down this frame's arena *before*
  releasing the parent storage Rc; on the shell, `storage` is declared before `scope_ptr`.
  Inner pointers die before the outer storage they may reference, ruling out a dangling
  `outer` during drop.

A scheduler slot's scope handle is lifetime-free, so the node carries no `'run` through its scope.
A per-call frame scope is stored as a payload-less
[`NodeScope::Yoked`](../src/machine/execute/nodes.rs) marker re-projected from the slot's own
`Node.frame` cart; a genuinely run-lived scope (a binder body's decl-scope child) is stored
as `NodeScope::Anchored`, an erased `ScopePtr<'static>` re-attached at read through `reattach_bounded`.
Both arms ride a grouped `NodePayload` (scope handle + lexical chain) alongside the slot's frame. The
slot-storage scope handle and the seed-side `with_frame_interior` re-anchor are documented in
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
- The running scope passes through `KoanRuntime::dispatch_in_scope(expr, scope)`
  directly, so dispatch sites carry their scope explicitly.
- [`RuntimeArena::alloc_function`](../src/machine/core/arena.rs) `debug_assert`s
  arena-identity between the function and its captured scope, catching a
  misallocated KFunction at the allocation site rather than later as a
  use-after-free in `lift_kobject`'s fast path.

## Performance notes

The push/notify scheduler ([execution-model.md § Push/notify dependency
edges](execution-model.md#pushnotify-dependency-edges)) keeps its slot-table
state in a
[`NodeStore`](../src/scheduler/node_store.rs)
sub-struct that owns `slots: SlotVec<SlotState<'run>>` (each slot a `PreRun(Node)`
/ `Running` / `Done(Result<Carried, KError>)` / `Aliased(NodeId)` / `Free`) and
`free_list: Vec<NodeId>`, behind the slot lifecycle
`alloc_slot → take_for_run → reinstall* → finalize → free_one`. `alloc_slot` is
the only path that picks an index (pulling from `free_list` before extending
`slots`), `finalize` is the only path that lands a terminal `Done`, and
`free_one` is the only path that returns a slot to `Free` and pushes its index
onto `free_list`. Dependency bookkeeping lives alongside it in a
[`DepGraph`](../src/scheduler/dep_graph.rs) sub-struct
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

Transient-node reclamation runs through `Scheduler::reclaim_deps` from the
unified node handler `KoanRuntime::run_step`, *after* the finish closure returns
its `Outcome` but *before* the harness applies it. So
when a dispatch splice finish has rewritten `working_expr.parts` to
`ExpressionPart::Future`, the freed indices are back on the free-list before
the harness dispatches the bound expression — its `add()` can recycle them
immediately. `reclaim_deps` clears `dep_edges[idx]` and
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
  frame reuse, MATCH `FrameStorage.outer` chain) is enumerated in
  [per-call-arena-protocol.md § Verification](per-call-arena-protocol.md#verification).
- The audit slate runs cycle-free across every unsafe site in the runtime
  under `MIRIFLAGS=-Zmiri-tree-borrows` with zero UB and zero process-exit
  leaks, signing off the memory model as it stands today. The canonical
  slate list lives in [observe/miri_slate.md](../observe/miri_slate.md).

## Open work

- **Scheduler owns every carrier reattach**
  ([refactor/scheduler-owns-carrier-reattach.md](../roadmap/refactor/scheduler-owns-carrier-reattach.md)).
  The value channel is already borrow-checked end to end in the scheduler; the
  continuation ([`run_loop.rs`](../src/machine/execute/run_loop.rs)) and contract
  ([`finalize.rs`](../src/machine/execute/finalize.rs)) reattaches still live in the
  driver against a free / prose-guarded lifetime. Fold both behind scheduler
  accessors that vend at a `'step` lifetime bounded by a witness `&Rc<W::Frame>` the
  caller already holds — no new `Rc` clone, so TCO's uniqueness gate is untouched —
  and rename `cont` → `continuation`.
