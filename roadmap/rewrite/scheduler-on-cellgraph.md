# Scheduler on cellgraph

The rewrite's bottom layer: the deferred-work scheduler Koan drives, built
directly over `cellgraph`'s cells and liveness matrix.

**Problem.** Koan's scheduler is `workgraph`'s DAG layer
([dag-scheduler.md](../../workgraph/old_design/dag-scheduler.md)), and that layer
sits on `workgraph`'s own cell substrate — the witnessed module with its
reference-counted pin bundles and antichain fold, and a node store
([node_store.rs](../../workgraph/src/scheduler/node_store.rs)) that interleaves
cell state with dep edges, park/notify bookkeeping, terminal delivery and
splicing. `cellgraph` ([cellgraph/README.md](../../cellgraph/README.md))
supplies the cell half with matrix liveness and tree cells, and nothing above it
uses it: the substrate the design settled on has no scheduler, and the scheduler
Koan runs on is the substrate the design moved off. The old runtime's execute
layer ([execute.rs](../../src/machine/execute.rs)) was shaped around that
scheduler's envelopes — `Delivered`, the finalize walk's carried reach, the
per-node `WorkingExpression` — so its step protocol is not a contract the
rewrite can restate; it is a record of what the old substrate required.

**Acceptance criteria.**

- A scheduler crate or module depends on `cellgraph` and on nothing above it;
  `cellgraph` depends on neither the scheduler nor `koan`.
- A unit of work is a `cellgraph` cell: its region, its erased continuation and
  its holds are the cell's, and the scheduler adds only dep edges, the work
  queue, the drain protocol and delivery.
- Liveness is the matrix's: no reference count, pin bundle or antichain fold
  exists in the scheduler, and a cell is reclaimed the instant no hold names it.
- A dep edge delivers by push (placement into the consumer during the
  producer's step) or by pull (a hold on the producer, read at the consumer's
  step); the finalize path carries no envelope of its own.
- A tail call is a release plus a create under the same node id, and a deep
  self-recursive loop runs at constant slab occupancy.
- A call subtree deeper than the slab cap runs on tree cells without touching
  the matrix.
- Admission at the slab cap is the drain's to handle; an embedder never sees a
  refused create as a hard error.
- The embedder contract names no Koan type: the scheduler's tests exercise it
  with a workload of its own, and its Miri slate is clean.

**Directions.**

- *A fresh crate, not `workgraph` rebuilt — decided.* The scheduler is written
  new against the cell contract, and `workgraph` is deleted once it stands.
  [adopt-cellgraph.md](../../workgraph/old_roadmap/adopt-cellgraph.md) planned the
  rebuild in place behind an expand/migrate/contract convention; that
  convention served the old runtime, and the pieces worth keeping — the edge
  slab, the drain protocol, alias splicing — are carried over by hand.
- *Where the step protocol lives — open.* Either the scheduler owns a
  `StepVerdict`-style protocol the embedder maps onto, as `workgraph` does, or
  the scheduler exposes only cell verbs and the embedder's decide/apply split
  is its own. Recommended: shape it against what a `values` delivery needs to
  carry, and do not restate the old `Outcome` enum.
- *Tree cells versus slab cells for a call — decided.* Per
  [cellgraph/src/tree/README.md](../../cellgraph/src/tree/README.md): a call subtree is a
  stack discipline and takes tree cells; slab cells are for work whose
  liveness is not nested.

## Dependencies

This item subsumes `workgraph`'s own
[adopt-cellgraph.md](../../workgraph/old_roadmap/adopt-cellgraph.md), which stays
as a requirements record for the fresh crate.

**Requires:** none — [values](../../src/values/README.md), what a cell delivers, ships.

**Unblocks:**

- [Scope on values and types](scope-on-values-and-types.md) — running a program through a scope needs the scheduler that drives it.
