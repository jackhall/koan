# Scheduler refactor phase 2 — Extract `DepGraph`

**Problem.** The three parallel dependency vectors on
[`Scheduler<'a>`](../src/runtime/machine/execute/scheduler.rs#L40-L53) —
`notify_list`, `pending_deps`, `dep_edges` — encode a tri-vector invariant
(every forward edge in `notify_list[p]` has a matching backward entry in
`dep_edges[c]` and contributes 1 to `pending_deps[c]`) that nothing in the
type system enforces. Two concrete failure surfaces follow:

- *Deferred-fixup gap.* [`run_dispatch`](../src/runtime/machine/execute/run/dispatch.rs#L70)
  and [`run_dispatch`'s replay-park branch](../src/runtime/machine/execute/run/dispatch.rs#L153)
  push `DepEdge::Notify` entries onto `dep_edges[idx]` raw, and
  [`defer_to_lift`](../src/runtime/machine/execute/run.rs#L33) pushes
  `DepEdge::Owned` raw; all three rely on a *later*
  [`register_slot_deps`](../src/runtime/machine/execute/scheduler/submit.rs#L106)
  call to fix up the other two vectors. A new edge-pushing path that
  forgets to call `register_slot_deps` desynchronizes the tri-vector
  silently.
- *Slot-recycle gap.* The slot-reuse arm of
  [`add`](../src/runtime/machine/execute/scheduler/submit.rs#L62-L68)
  clears all three vectors in three separate statements; nothing stops a
  future caller from resetting one and forgetting the others, leaving the
  recycled slot to inherit stale forward or backward edges.

The notify-walk in
[`notify_consumers`](../src/runtime/machine/execute/scheduler/execute.rs#L143)
decrements `pending_deps` and chooses which consumers became ready; the
"every decrement either keeps the consumer parked or routes it to
`ready_set`" rule is the second tri-vector invariant. It survives today as
a single function but is not the only path that could mutate
`pending_deps` — nothing prevents a future caller from decrementing
`pending_deps[c]` from somewhere else.

**Impact.**

- *Edge-addition invariant type-enforced.* Two methods, `add_owned_edge`
  and `add_park_edge`, become the only edge-addition paths from outside
  the struct. Both mutate `notify_list[p]`, `pending_deps[c]`, and
  `dep_edges[c]` together. The deferred-fixup gap in `run_dispatch` and
  `defer_to_lift` closes — `register_slot_deps`'s "patch up after the
  raw push" responsibility disappears.
- *Slot-recycle invariant type-enforced.* `reset_slot_deps(idx,
  owned_edges)` is the only path that resets a slot's dep bookkeeping; it
  resets all three vectors together. Slot recycling resets the three
  vectors as one operation.
- *Notify-walk invariant type-enforced.* `drain_notify(idx) -> Vec<usize>`
  is the only path that decrements `pending_deps`; it returns the woken
  consumers in one move. The caller (`Scheduler::finalize`, landing in
  phase 3) routes the returned consumers through
  `WorkQueues::push_woken`. Every `pending_deps[c]` decrement is paired
  with the routing decision for `c`.
- *Cascade-free walk invariant type-enforced.* `owned_children(idx)`
  returns an iterator over `Owned`-tagged entries in `dep_edges[idx]`.
  `Scheduler::free` (cross-struct, stays on `Scheduler`) calls this and
  orchestrates its own iterative stack-walk. The cascade walk only
  yields owned children — `Notify` edges are filtered inside `DepGraph`.

**Directions.**

- *Sub-struct introduction — decided.* Add `DepGraph` as a sibling module
  under
  [`src/runtime/machine/execute/scheduler/`](../src/runtime/machine/execute/scheduler/).
  Three private fields (`notify_list`, `pending_deps`, `dep_edges`); the
  only mutation paths are the wrapper methods listed below.
  `Scheduler<'a>`'s three dep fields are replaced by a single
  `deps: DepGraph` field in the same edit.
- *Wrapper surface — decided.*
  `add_owned_edge(producer, consumer)`,
  `add_park_edge(producer, consumer)`,
  `drain_notify(idx) -> Vec<usize>`,
  `reset_slot_deps(idx, owned_edges)`,
  `owned_children(idx) -> impl Iterator<Item = NodeId>`. Plus the
  growth helper `extend_for_new_slot()` that pushes empty entries onto all
  three vectors when `add` extends rather than recycles (its index-space
  pair on `NodeStore` lands in phase 3).
- *`add_owned_edge` / `add_park_edge` vs single `add_edge(kind, ...)` —
  decided.* Two named methods, not a single `add_edge(kind, ...)`. The
  call sites that push park edges (`run_dispatch:70`, `run_dispatch:153`)
  and the call site that pushes owned edges (`defer_to_lift:33`) know
  their kind statically; no caller branches on edge kind. Two named
  methods make each call site's intent self-documenting and prevent
  kind-mixups at the type level. The choice does not extend the wrapper
  surface meaningfully (one method body each, both trivially short).
- *`work_owned_edges` move — decided.* The canonical
  owned-edges builder in
  [`nodes.rs`](../src/runtime/machine/execute/nodes.rs#L114)
  becomes a `DepGraph` associated function (or feeds
  `reset_slot_deps`'s constructor argument). It moves to live next to
  `reset_slot_deps`.
- *Call-site migration — decided.* All three raw `dep_edges[idx].push(...)`
  sites convert in the same commit that introduces `DepGraph`:
  - `run/dispatch.rs:70` (`DepEdge::Notify(producer_id)` for bare-name
    short-circuit) → `self.deps.add_park_edge(producer_id, NodeId(idx))`.
  - `run/dispatch.rs:153` (`DepEdge::Notify(*p)` for replay-park) → same.
  - `run.rs:33` (`DepEdge::Owned(bind_id)` in `defer_to_lift`) →
    `self.deps.add_owned_edge(bind_id, NodeId(idx))`.

  `register_slot_deps`'s notify-edge install responsibility for these
  three sites disappears (`add_owned_edge` / `add_park_edge` install the
  forward edge directly and increment `pending_deps[idx]` on the spot);
  `register_slot_deps` continues to walk `dep_edges[idx]` only for the
  initial edge set built by `work_owned_edges` at `add()` time. The
  slot-reuse arm of `add` switches to
  `self.deps.reset_slot_deps(i, owned_edges)`; the extend arm switches to
  `self.deps.extend_for_new_slot()`. The notify-walk inside
  `notify_consumers` (still on `Scheduler` until phase 3) calls
  `self.deps.drain_notify(idx)` and routes the result through
  `self.queues.push_woken(_)`. `free`'s edge-walk in
  [`execute.rs:171`](../src/runtime/machine/execute/scheduler/execute.rs#L171)
  switches to `self.deps.owned_children(i)`; the underlying
  `dep_edges[i]` take + edge filter lives inside `DepGraph` now.
- *Cross-struct composition — decided.* `Scheduler::notify_consumers`
  (a one-line orchestration: `for c in self.deps.drain_notify(idx) {
  self.queues.push_woken(c) }`) stays on `Scheduler` and survives this
  phase. It folds into `Scheduler::finalize` in phase 3 once
  `NodeStore::finalize` exists. No transitional shim — both
  sub-structs see only their own fields.
- *No `pub(super)` shim on `DepGraph` — decided.* Every raw mutation site
  is converted in the same commit, so no half-migrated state exists where
  a caller still needs raw field access. Verified by the call-site list
  above.
- *Verification — decided.* `cargo build`, `cargo test`, and `cargo clippy
  --all-targets` all pass. The existing notify-walk semantics tests in
  [`scheduler/tests.rs`](../src/runtime/machine/execute/scheduler/tests.rs)
  exercise `add_dispatch` + `execute` end-to-end and continue to pass
  unchanged. Optionally run `tools/modgraph.py` to confirm the
  partition-complexity score improves (`Scheduler`'s field count drops
  from 9 to 5; one new internal module).

## Dependencies

**Requires:**

**Unblocks:**
- [Scheduler refactor phase 3 — Extract `NodeStore`](scheduler-3-nodestore.md) —
  `NodeStore::finalize`'s natural shape composes with
  `DepGraph::drain_notify`; designing `NodeStore::finalize` before
  `DepGraph`'s API is fixed risks churn.
