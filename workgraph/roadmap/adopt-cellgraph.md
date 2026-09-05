# Rebuilding workgraph over cellgraph

**Problem.** `workgraph` owns its own cell substrate. The witnessed module
([witnessed.rs](../src/witnessed.rs)) tracks liveness with reference-counted
pin bundles, a `PinsRegion` hook, and an antichain fold
([reach.md](../design/reach.md)); the slot table in
[node_store.rs](../src/scheduler/node_store.rs) interleaves cell state —
region, erased continuation, anchor — with DAG-only state: `SlotState`
terminality, dep edges, notify and park bookkeeping, terminal delivery,
splicing. The `cellgraph` crate
([design/cellgraph.md](../../cellgraph/design/cellgraph.md)) supplies the cell
half with matrix liveness, and `workgraph` does not use it: two substrates
exist, and koan sits on the one the design has moved off.

**Acceptance criteria.**

- `workgraph` depends on `cellgraph`, and `cellgraph` depends on neither
  `workgraph` nor `koan`.
- `Workload` names the cell contract's two embedder types plus the terminal
  error type; there is no frame or anchor type in the trait.
- Node identity is a `workgraph` id mapped onto a `cellgraph` handle, and a
  tail-call reinstall is a release plus a create under the same node id; the
  TCO suite in koan passes with constant slab occupancy across a deep loop.
- Each dep edge delivers by push (mint or destination-homed placement into
  the consumer during the producer's step) or by pull (a bare hold on the
  producer, read sealed at the consumer's step); the choice is a per-edge
  attribute the drain applies, and the finalize walk holds no envelope of its
  own.
- The embedder can give a node a storage-only companion cell — a cart that
  outlives the node's per-step body cells — through the consumer API, and a
  koan loop that accumulates into one runs with per-hop body cells dying at
  row zero.
- Admission is `workgraph`'s: a create refused at the slab cap is handled by
  the drain, never surfaced to the embedder as a hard error.
- `workgraph`'s witnessed module, `PinsRegion`, pin bundles, the `Delivered`
  envelope, and the reference-counted retention path are deleted; the edge
  slab, park/notify, the drain protocol, the finalize walk, and alias
  splicing remain and are the only scheduler state.
- Koan compiles and its full slate passes on the rebuilt crate, and the
  Miri slate is clean.

**Directions.**

- *Slot-table split — decided.* Composition: DAG state lives in a
  `workgraph` table keyed by node id, holding the cell handle; the cell table
  keeps no DAG vocabulary.
- *Migration shape — open.* (a) rebuild in place behind the existing
  consumer API, landing as one expand-migrate-contract sequence per
  [the roadmap convention](README.md#crossing-the-crate-boundary); (b) a
  second scheduler module over `cellgraph` grown alongside the old one until
  koan switches. Recommended: (a); koan names the carrier types directly, so
  there is no facade to hide (b) behind.
- *Per-edge push/pull default — open.* Push wherever the crossing rule
  applies, pull otherwise; or pull everywhere first and add push as an
  optimization. Recommended: push where the crossing rule applies, since
  that is the shape that keeps per-call cells out of the sealed tier.

## Dependencies

**Requires:**

- [The cell substrate](../../cellgraph/roadmap/cell-substrate.md) — the pull
  shape and the loop-cart shape both need seals.

**Unblocks:**

- [Publishing the workgraph crate](workgraph-extraction.md) — the published
  boundary is the layered pair.
