# Stateful dispatch — Step 2: `recent_wakes` side-channel

Add the wake-time signal that tells a stateful `Dispatch` consumer
*which* producer fired since its last poll. The new driver looks up
per-edge callbacks in `DispatchState` keyed by producer `NodeId`, so
the woken slot needs to know what to drain.

**Problem.** Today's
[`Scheduler::finalize`](../../src/machine/execute/scheduler/execute.rs)
drains a producer's `notify_list`, decrements each consumer's
`pending_deps`, and pushes the consumer onto the run-set when its
counter hits zero — but the consumer is never told *which*
producer fired. `Bind` / `Combine` / `Catch` don't need that
information because they run a fixed closure on counter-zero. The
stateful `Dispatch` driver does need it: per-edge callbacks live in
`DispatchState` keyed by producer `NodeId`, and without a wake-time
signal the woken slot can't pick the right callback.

**Impact.**

- Each `Dispatch` consumer slot carries a private `Vec<NodeId>` of
  producers that have fired since its last poll. The woken slot
  drains this on entry and runs each callback against its
  `DispatchState`. Non-`Dispatch` consumers (`Bind`, `Combine`,
  `Catch`) are unaffected — `notify_list` semantics for them are
  unchanged.
- The side-channel's storage matches the existing
  `notify_list` / `dep_edges` ownership shape: an outer
  `Vec<Vec<NodeId>>` indexed by consumer slot, recycled through
  `NodeStore::free_one`, so it adds no new growth pattern. Inner
  Vec capacity persists across slot reuse to amortize allocation.
- Step 3 and step 4 can implement per-variant handlers against a
  ready signal without redefining the producer/consumer surface.

**Directions.**

- **Storage shape — decided.** `recent_wakes: Vec<Vec<NodeId>>` on
  [`NodeStore`](../../src/machine/execute/scheduler/node_store.rs),
  indexed by consumer slot. Per-consumer, not per-producer: a
  producer with two `Dispatch` consumers fires `append` against
  each consumer's `recent_wakes[c]` independently. Dual of
  `notify_list[p] = Vec<consumer>`.

- **Population — decided.**
  [`Scheduler::finalize`](../../src/machine/execute/scheduler/execute.rs)
  walks the producer's `notify_list`; for each consumer `c`, if
  `nodes[c].work` is `NodeWork::Dispatch { state: DispatchState::
  Initialized(_) | … }`, append the producer's `NodeId` to
  `recent_wakes[c]` before the existing `pending_deps[c] -= 1` and
  the `push_woken` on counter-zero. Non-`Dispatch` work variants
  skip the append. The discriminator check matches the existing
  `stamp_lift_ready` pattern (peek the work variant during
  finalize).

- **Drain — decided.** `run_dispatch_stateful` takes (not peeks)
  `recent_wakes[idx]` on entry via
  `NodeStore::take_recent_wakes(idx) -> Vec<NodeId>`. The Vec's
  length resets to zero; capacity is retained for the next batch.

- **Cleanup — decided.** `NodeStore::free_one` calls
  `recent_wakes[idx].clear()`. O(1) per slot reclamation; matches
  the existing free-time clears on `notify_list[idx]` and
  `dep_edges[idx]`. Capacity is retained so a recycled slot
  inherits the prior allocation.

- **API surface — decided.** Two new methods on `NodeStore`:
  `push_recent_wake(consumer: NodeId, producer: NodeId)` and
  `take_recent_wakes(consumer: NodeId) -> Vec<NodeId>`. Internal
  callers only; the field is private and gated by the same
  atomic-mutator discipline as `nodes` / `results` / `free_list`.

- **Acceptance criteria — decided.** `cargo test` green with
  toggle on and off. Add a unit test asserting `take_recent_wakes`
  returns empty for a non-`Dispatch` consumer (the side-channel
  is `Dispatch`-only by construction). The field stays empty in
  practice this step — no consumers query it yet.

- **Bounded-growth confirmation — decided.** The outer Vec grows
  with peak slot count, same as `nodes` / `results` /
  `notify_list` / `dep_edges`. Inner Vec length resets to zero on
  every drain; capacity grows to the peak wake-batch for that
  slot (small in practice — one for single-park, a few for
  combined-park forward references). A Dispatch can't accumulate
  unbounded wakes without polling: each wake fires via
  `pending_deps == 0` → `push_woken`, which guarantees a poll
  consumes the wakes before the next batch can accumulate.

## Dependencies

**Requires:**

- [Stateful dispatch — Step 1: scaffolding](stateful-dispatch-01-scaffolding.md)

**Unblocks:**

- [Stateful dispatch — Step 3: fast-lane variants](stateful-dispatch-03-fast-lane-variants.md)
