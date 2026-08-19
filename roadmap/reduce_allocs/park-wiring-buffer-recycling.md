# Park-wiring buffer recycling

**Problem.** Wiring one park allocates five vectors, and a recycled scheduler slot throws
away the capacity a previous incarnation grew.

On the koan side, `wire_deps`
([src/machine/execute/harness.rs](../../src/machine/execute/harness.rs)) resolves deps
through `named_sources`, which returns a `(sources, minted)` pair of `Vec<EdgeId>`, and
`install_deps` returns a `Vec<InstalledEdge>`; the block arm `block_sources` returns two
more plus the `Vec<NodeId>` its fan-out produced. `install_eager_subs`
([src/machine/execute/decide/keyworded.rs](../../src/machine/execute/decide/keyworded.rs))
adds churn of its own: it `unzip()`s the staged subs into two vectors, then
`Deps::from_requests` pushes one of them into a third. The `unzip`/`from_requests` pair
produces nothing the staging walk did not already have.

On the workgraph side, `install_for_slot` assigns a fresh `ResolvedDeps` and
`take_deps` / `take_notify` `mem::take` their vectors
([workgraph/src/scheduler/dep_graph.rs](../../workgraph/src/scheduler/dep_graph.rs)), so a
slot recycled off the free list drops the buffer its predecessor grew and starts from
zero. Mirroring `dep_sources`, the drain's own `dep_results`
([workgraph/src/scheduler/drain.rs](../../workgraph/src/scheduler/drain.rs)) means a
parked slot pays two vectors per wake.

**Acceptance criteria.**

- Staging eager subs builds its `Deps` in the walk that produces the requests: no
  `unzip` into two vectors and no re-push into a third.
- A recycled scheduler slot reuses its dep and notify row vectors — cleared, not
  replaced — so a steady-state drain over a fixed graph shape allocates no new row
  vector after warm-up.
- The per-park `Vec<EdgeId>` buffers are drawn from a reusable store rather than freshly
  allocated, and the store returns them cleared.
- The recorded allocation count for a program that parks and wakes the same slot shape
  repeatedly is constant after the first park.

**Directions.**

- *Mechanism — decided.* A reusable buffer store, not the scratch arena. These sites sit
  on the harness side, past the step brand, and their contents (`Vec<EdgeId>`,
  `Vec<NodeId>`, `Vec<InstalledEdge>`) are fixed `Copy` repeat shapes — the case a pool
  of clearable buffers covers and an arena does not, since the arena's confinement story
  depends on a `'b` that is not in scope here.
- *`Deps`' allocator — open.* Whether `Deps<R>`
  ([workgraph/src/scheduler/deps.rs](../../workgraph/src/scheduler/deps.rs)) grows an
  allocator parameter or stays global-allocator-backed. It is workgraph API and its
  contents escape the decide as park data, so it cannot ride a step-scoped arena either
  way. Recommended: leave it global and recycle the slot rows instead — the escape makes
  the allocator parameter buy little.
- *Where the pool lives — open.* On `Host` for the koan-side buffers versus on the
  scheduler for the row vectors. The two halves can land independently; the workgraph
  half is verified by `cargo +nightly miri test -p workgraph --lib`.

## Dependencies

The scratch arena ([Step-scoped scratch arena](step-scratch-arena.md)) is deliberately
not the mechanism here — see the first Directions bullet — so the two items are
independent and may land in either order.

**Requires:** none.

**Unblocks:** none.
