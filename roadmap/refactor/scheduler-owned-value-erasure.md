# Scheduler-owned value erasure via a `'node` lifetime

Move the generic erase/reattach of inter-node values into the scheduler, so a
slot read hands back a borrow-checked live value instead of an opaque erased blob
the driver re-anchors with `unsafe`.

**Problem.** Moving a value along a dep edge is the scheduler's job, but the
erase/reattach machinery that makes it lifetime-safe lives outside the scheduler
and is executed at both ends by the Koan driver:

- The generic carrier owner — [`Erased<T>`](../../src/machine/core/reattach.rs)
  over `unsafe trait Reattachable { type At<'r>; }`, plus the `reattach_value` /
  `reattach_ref` / `reattach_slice` helpers — lives in `machine::core::reattach`,
  yet names no Koan type. The [scheduler](../../src/scheduler/workload.rs) imports
  nothing from `machine`; it stores the value opaquely as `Workload::Value`.
- The driver erases on the way in — `result.map(ErasedValue::erase)` at the
  [`finalize`](../../src/machine/execute/run_loop.rs) call site — and reattaches on
  the way out: the [`KoanRuntime`](../../src/machine/execute/runtime.rs) read
  forwarders (`read_result` / `read` / `read_lifted`) and the
  [`outcome.rs`](../../src/machine/execute/outcome.rs) `pin_carried_to_run` /
  `deps_for_builtin` helpers each fabricate `Carried<'run>` from the opaque blob
  with an `unsafe` reattach.
- The witness that makes that reattach sound is *already in the scheduler*:
  `SlotState::Done(Result<W::Value, _>, Option<Rc<W::Frame>>)`
  ([node_store.rs](../../src/scheduler/node_store.rs)) co-stores the producer frame
  `Rc` beside the value. The scheduler holds the pin but hands back the unpinned
  blob, leaving the driver to assert — in a SAFETY comment, not the type system —
  that the frame outlives the fabricated `'run`.

So the value-movement boundary is split: the scheduler owns the storage and the
pin, the driver owns the erase and the lifetime fabrication, and the generic
machinery sits in a third place (`machine::core`).

**Acceptance criteria.**

- `Reattachable`, `Erased<T>`, and the `reattach_value` / `reattach_ref` /
  `reattach_slice` helpers live under `src/scheduler/`; `machine` depends on the
  scheduler for them (the families — `CarriedFamily`, `KObjectFamily`, … — impl the
  scheduler's trait beside their own types). The scheduler still names no Koan value,
  scope, memory, or AST type.
- The scheduler's value reads (`read_result` / `read` / `read_result_with_frame`)
  return a `'node`-bounded live value (`W::Value::At<'node>`), where `'node` is the
  scheduler's own `&self` borrow — not an opaque `Workload::Value` the caller
  re-anchors.
- The value reattach is borrow-checked: `'node` is bounded by the borrow under which
  the slot's frame `Rc` cannot be dropped (`free_one` needs `&mut self`), so the
  pin-outlives-read fact today carried by a SAFETY comment becomes a borrow the
  compiler checks.
- The driver's *transient* reads (`KoanRuntime::read_result` / `read` and the
  `SchedulerView::read_result` forwarder) perform no `unsafe` reattach — they consume
  the scheduler's safe `'node` value directly. The `'run`-fabricating reads that feed
  the lift and contract hooks (`read_lifted`, `pin_carried_to_run`) are out of scope
  here — they need a node-lifetime rethread of those hooks, tracked separately.
- The driver no longer erases a terminal before `finalize`: it hands `finalize` the
  live step value and the scheduler erases it internally (`ErasedValue::erase` at the
  call site is gone).
- `pin_deref`, [`ScopePtr`](../../src/machine/core/scope_ptr.rs), and the arena
  self-referential derefs stay in `machine::core` — they recover a pointer whose
  pointee an arena pins, not a value moving between nodes.
- The continuation reattach
  ([`run_loop.rs`](../../src/machine/execute/run_loop.rs)) stays driver-side: at run
  time its slot is `Running` and the witnessing cart is held by the driver's step
  guard, not a live slot, so there is no scheduler borrow to bind a `'node` to.

**Directions.**

- *Thread a `'node` lifetime through the read surface — decided.* The scheduler
  reattaches internally and returns `W::Value::At<'node>` bounded by `&'node self`,
  witnessed by the frame `Rc` the `Done` slot already holds. `Workload::Value` gains a
  `Reattachable` bound (its `Value: Copy` requirement becomes
  `for<'r> Value::At<'r>: Copy`); Koan binds `Value::At<'r> = Carried<'r>`.
- *A scheduler-owned node arena instead — rejected.* Koan terminals are cyclic
  `KObject` graphs born in per-call `CallArena`s that free with their producer slot;
  a scheduler-owned arena would copy every terminal out at `finalize` and lose
  per-node reclamation (a monotonic arena can't free one node's memory). The
  frame-`Rc`-in-slot is already the per-node pin — a node arena buys nothing the
  `'node` thread doesn't and regresses the memory model.
- *Home module for the moved generic — decided.* A new `scheduler/erase.rs` owns
  `Reattachable` / `Erased` / the transient helpers; `machine::core::reattach` keeps
  only `pin_deref` (the arena-pointer re-borrow, which is not value movement).
- *Lift and contract re-anchor — deferred.* `read_lifted` and `pin_carried_to_run`
  still fabricate `'run` because `NodeLift::lift` and `NodeFinalize::finalize_terminal`
  are typed at one collapsed `'run`. Threading node lifetimes through those hooks is its
  own change, tracked in
  [node-lifetime lift and contract re-anchor](node-lifetime-lift-and-contract.md); this
  item leaves both reattaches in place and only moves the transient-read and erase
  ownership.
- *`read_result_with_frame` keeps its framed/frameless query — decided.* Consumer-pull
  lift needs to know whether to copy (framed) or forward (frameless run-arena value), so
  the read returns the `'node` value plus the frame `Rc`; the driver branches on it.

## Dependencies

A refactor-hygiene item on the value-movement seam between the scheduler and the
Koan driver; update [design/memory-model.md § Arena lifetime erasure](../../design/memory-model.md#arena-lifetime-erasure)
and [design/execution-model.md § The dispatcher / scheduler boundary](../../design/execution-model.md#the-dispatcher--scheduler-boundary)
if the erasure ownership it describes changes. Both prerequisites — the
workload-independent DAG runtime and the unified `Erased<T>` carriers — have shipped.

**Requires:** none — both prerequisites shipped.

**Unblocks:**
- [Node-lifetime lift and contract re-anchor](node-lifetime-lift-and-contract.md) — the `'node` read
  surface and `Erased<W::Value>` store are the substrate that rethread extends to the lift / Done hooks.
