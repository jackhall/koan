# Park-wiring buffer recycling

**Problem.** Wiring one park allocates a run of transient vectors, and a recycled
scheduler slot throws away the capacity a previous incarnation grew.

On the koan side, `wire_deps`
([src/machine/execute/harness.rs](../../src/machine/execute/harness.rs)) resolves deps
through `named_sources`, which returns a `(sources, minted)` pair of `Vec<EdgeId>`, and
`install_deps` returns a `Vec<InstalledEdge>`; the block arm `block_sources` returns two
more, plus the `Vec<NodeId>` its fan-out produced and the `Vec<WorkingExpression>`
`split_working_body` splits a block into. `install_eager_subs`
([src/machine/execute/decide/keyworded.rs](../../src/machine/execute/decide/keyworded.rs))
adds churn of its own: it `unzip()`s the staged subs into two vectors, then
`Deps::from_requests` pushes one of them into a third. The `unzip`/`from_requests` pair
produces nothing the staging walk did not already have.

On the workgraph side, `install_for_slot` assigns a fresh `ResolvedDeps` and
`take_deps` / `take_notify` `mem::take` their vectors
([workgraph/src/scheduler/dep_graph.rs](../../workgraph/src/scheduler/dep_graph.rs)), so a
slot recycled off the free list drops the buffer its predecessor grew and starts from
zero. The drain allocates a fresh `dep_results` vector per wake
([workgraph/src/scheduler/drain.rs](../../workgraph/src/scheduler/drain.rs)) and the
delivery walk a `woken` vector per finalize
([workgraph/src/scheduler/lifecycle.rs](../../workgraph/src/scheduler/lifecycle.rs)).
(`Workload::retiring` is not on this list: koan's impl is a `mem::take` of the anchor's
owned-edge record and the default returns `Vec::new()` — zero allocations per verdict —
so it stays as it is.)

Every buffer above except the slot rows is born and consumed within one drain pop. The
rows alone are park data — alive from install to wake — so they are the only class the
step scratch arena cannot host.

**Acceptance criteria.**

- Staging eager subs builds its `Deps` in the walk that produces the requests: no
  `unzip` into two vectors and no re-push into a third.
- `Deps<R>` carries an allocator parameter: a park built inside a step hosts its entries
  on the scratch arena at the step's own brand, and submission paths outside a step keep
  the global-allocator default.
- A recycled scheduler slot reuses its dep and notify row vectors — cleared, not
  replaced — so a steady-state drain over a fixed graph shape allocates no new row
  vector after warm-up.
- The step-transient wiring buffers — `sources`/`minted`, the block fan-out's
  `Vec<NodeId>` and `Vec<WorkingExpression>`, `install_deps`' verdicts, the drain's
  `dep_results`, and finalize's `woken` — are hosted on the step scratch arena rather
  than freshly heap-allocated.
- The recorded allocation count for a program that parks and wakes the same slot shape
  repeatedly is constant after the first park — measured scheduler-side, by a workgraph
  counting-allocator test over a fixed park/wake shape; the koan-level
  `tests/allocation_baseline.rs` bounds are re-measured and lowered deliberately
  alongside it.

**Directions.**

- *Mechanism — decided.* The step scratch arena the drain owns and resets once per pop,
  handed to the step callback on `Step::scratch`
  ([workgraph/src/scheduler/drain.rs](../../workgraph/src/scheduler/drain.rs)). Every wiring buffer above is born and consumed within one pop — the
  koan-side sites included, since `wire_deps` runs inside `apply`, inside the step — so
  the arena's confinement covers them. The slot rows are recycled in place instead: they
  are cross-pop park data, outside any arena's reach.
- *Row-recycling shape — decided: take-and-restore.* The take verbs get restore
  counterparts that hand the buffer back to its own row, cleared — capacity stays owned
  by the row that grew it, and the drain keeps no buffer state. All three consumers
  (`drain`'s dep read, `finalize`'s walk, `splice_forward`) must move the vector out to
  call `&mut self` methods anyway; the dep buffer is restored *before* the step callback
  runs, so a mid-step `Replace` installs into the recycled buffer.
- *`Workload::retiring` — decided: unchanged.* Zero allocations per verdict already (see
  the Problem note), so there is nothing to recycle.

The implementation plan is `scratch/park-wiring-buffer-recycling-plan.md` (untracked).

## Dependencies

**Requires:** none — the step scratch arena this item hosts its buffers on is shipped: the
drain owns the bump and hands its handle out on `Step::scratch`
([workgraph/src/scheduler/drain.rs](../../workgraph/src/scheduler/drain.rs)), described in
[dag-scheduler.md § The drain protocol](../../workgraph/design/dag-scheduler.md#the-drain-protocol).

**Unblocks:** none.
