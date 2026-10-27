# Run cells alone

Cellgraph's surface now that no build cell is a cell.

**Problem.** Cellgraph grew doors for one embedder's build tree, which koan
now keeps outside the graph in
[its build arena](../../src/scheduler/README.md#the-build-arena):
`enter_beside`, `peek` and `is_childless` have no koan caller, and the named
and copied lends of [`lend.rs`](../src/lend.rs), counted tenant lends with
their `held`/`unheld` books, [`lend_cycle`](../src/graph/lend_cycle.rs),
and the `'lent` brand that gives `StepContext` ten
parameters ([`graph.rs`](../src/graph.rs)) remain. A `Fresh` koan frame still
takes a counted lend of its parent's scratch that no chain reads
([the drain](../../src/scheduler/drain.rs)).

**Acceptance criteria.**

- Cellgraph's public surface holds no `enter_beside`, `peek`, `is_childless`,
  `Lend`, `Lent` or `'lent`, no counted tenant lend, no `held`/`unheld`
  books and no `lend_cycle`, and
  [`tests/surface.rs`](../tests/surface.rs) names what remains.
- `StepContext` and koan's `Step` carry no `'lent`; a `Fresh` frame takes no
  lend; a tenant's release passes no count to its host's scratch, and the
  Miri slate tests that pinned those passes go with them.
- [Tree cells](../src/tree/README.md) and [lifetimes](../src/lifetimes.md)
  describe no lend.

**Directions.**

- *A tenant's own scratch — decided.* Not needed: a tenant borrows nothing,
  so nothing is counted on its host's scratch and no lend outlives a release.
- *The header projection — decided.* `Projects`/`Projected` stay: koan's
  [region header](../../src/scope/header.rs) holds a half's decision memo
  beside its type branch, and `values` reads the branch through the projection.

## Dependencies

**Requires:** none.

**Unblocks:** none — a leaf.
