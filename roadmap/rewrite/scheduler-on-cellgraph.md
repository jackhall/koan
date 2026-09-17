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
- A deferred-only component of a body's bindings
  ([src/scope/README.md](../../src/scope/README.md#visibility)) is one unit of
  work: one cell claims every member's slot at submission, a refused tie
  ([src/function/README.md](../../src/function/README.md#the-tie)) naming a pending binder becomes a
  dep edge on that binder's cell and the step re-runs when it delivers, a
  refused tie naming an eager part of a data member becomes a sub-dispatch
  whose value the step supplies to the tie by site when it re-runs, and the
  one tie that succeeds binds every member's slot from the knot it hands back.
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

- *Own region versus the caller's region for a sub-dispatch — decided.*
  `cellgraph` ships a tenant cell — one with no region of its own, whose writer
  is its host's — and the scheduler elects it per spawned cell from a one-bit,
  per-function hint: whether the function's result shares structure with its
  arguments. A fresh result is built in a tree child with its own region and
  placed operand-free into its destination, so the child reclaims whole at
  death; a sharing result is built by a tenant of the caller's frame, which
  embeds its arguments at no price. The same bit decides a tail hop: a fresh
  hop draws a recycled region and the loop runs in bounded memory, a sharing
  hop joins its predecessor's region, whose growth the result retains anyway.
  The hint is never a contract — a wrong *fresh* costs a priced copy, a wrong
  *shares* costs delayed reclaim, and soundness rests on the substrate's brands
  either way. First cut: builtins declare the bit and a user function derives
  it from its return type (a flat return cannot share). Open beyond the first
  cut, in precedence order over the derived bit: a programmer annotation with
  no semantic effect, since the cost of a wrong guess scales with data size
  only a programmer can predict; and a runtime measurement of copied and
  retained bytes per function. A sharing tail loop accumulates per-hop garbage
  in its region, so a scratch habitat scoped to the shared region comes with
  tenancy.

## Dependencies

This item subsumes `workgraph`'s own
[adopt-cellgraph.md](../../workgraph/old_roadmap/adopt-cellgraph.md), which stays
as a requirements record for the fresh crate.

[values](../../src/values/README.md), what a cell delivers, ships.

**Requires:**

- [Shared regions, a scratch habitat and region recycling](../../cellgraph/roadmap/shared-regions.md) — tenant cells, per-region scratch and the recycled tail hop.

**Unblocks:**

- [Dispatch](dispatch.md) — running a program needs the scheduler that drives it.
- [Yielding iterators](yielding-iterators.md) — a flat consumer loop is its tail call.
