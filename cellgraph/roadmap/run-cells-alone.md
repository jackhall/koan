# Run cells alone

Cellgraph's surface once no build cell is a cell.

**Problem.** Cellgraph grew doors for one embedder's one tree, each with one
caller or none: `enter_beside`, `peek`, `is_childless`, the named and copied
lends of [`lend.rs`](../src/lend.rs), counted tenant lends with their
`held`/`unheld` books, [`lend_cycle`](../src/graph/lend_cycle.rs),
`Projects`/`Projected`, and the `'lent` brand that gives `StepContext` ten
parameters ([`graph.rs`](../src/graph.rs)). `peek` is documented as a read for
a drain, yet koan writes header cells through it; `peek` refuses a tenant
while `enter_beside` maps one to its host; a `Fresh` koan frame still takes a
counted lend of its parent's scratch that no chain reads
([the drain](../../src/scheduler/drain.rs)). Koan's header family carries a
table, a home and settled facts on every run cell for a build cell that
[the build arena](../../roadmap/gradual-build/build-arena.md) moves out of the
graph.

**Acceptance criteria.**

- Cellgraph's public surface holds no `enter_beside`, `peek`, `is_childless`,
  `Lend`, `Lent` or `'lent`, no counted tenant lend, no `held`/`unheld`
  books, no `lend_cycle` and no `Projects`/`Projected`, and
  [`tests/surface.rs`](../tests/surface.rs) names what remains.
- Koan's header family holds a half's type branch alone; no run cell's header
  carries a table, a home or settled facts.
- The graph holds one slab cell, the run root; the load is no cellgraph work.
- `StepContext` and koan's `Step` carry no `'lent`; a `Fresh` frame takes no
  lend; a tenant's release passes no count to its host's scratch, and the
  Miri slate tests that pinned those passes go with them.
- [Tree cells](../src/tree/README.md) and [lifetimes](../src/lifetimes.md)
  describe no lend.

**Directions.**

- *A tenant's own scratch — decided.* Not needed: a tenant borrows nothing,
  so nothing is counted on its host's scratch and no lend outlives a release.
- *Order — decided.* After the arena ships, since koan's frames reach their
  build cells through the arena's own reference.

## Dependencies

**Requires:**

- [The build arena](../../roadmap/gradual-build/build-arena.md) — the build
  leaves the cell graph before its doors go.

**Unblocks:** none — a leaf.
