# Rebuilding workgraph over cellgraph

**Problem.** `workgraph` owns its own cell substrate. The witnessed module
([witnessed.rs](../src/witnessed.rs)) tracks liveness with reference-counted
pin bundles, a `PinsRegion` hook, and an antichain fold
([reach.md](../old_design/reach.md)); the slot table in
[node_store.rs](../src/scheduler/node_store.rs) interleaves cell state —
region, erased continuation, anchor — with DAG-only state: `SlotState`
terminality, dep edges, notify and park bookkeeping, terminal delivery,
splicing. The `cellgraph` crate
([old_design/cellgraph.md](../../cellgraph/design/cellgraph.md)) supplies the cell
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
  column zero.
- Admission is `workgraph`'s: a create refused at the slab cap is handled by
  the drain, never surfaced to the embedder as a hard error.
- `workgraph`'s witnessed module, `PinsRegion`, pin bundles, the `Delivered`
  envelope, and the reference-counted retention path are deleted; the edge
  slab, park/notify, the drain protocol, the finalize walk, and alias
  splicing remain and are the only scheduler state.
- Koan compiles and its full slate passes on the rebuilt crate, and the
  Miri slate is clean.
- The kind rule is an admission decision at creation: a cell whose source
  edge is destined at its creator's region, or at a
  [tree cell](../../cellgraph/design/tree-cells.md) under the same root, is a
  tree cell; top-level statements, yielding producers, and any cell an outside
  consumer can pin while it lives are slab cells. The substrate ships both
  kinds and no rule for choosing between them.
- The delivery walk adopts a tree terminal once, into its canonical
  destination — the producer's own source edge's region, the shallowest on
  the chain — by the placement door whose `Pin` pledges the producer, and the
  deeper destination buckets redeem that dormant carrier by reference.
- A non-tail recursion deeper than the slab cap runs to completion; the koan
  program that refuses admission today is the regression test, and a test pins
  the chain property — a closure returned out of a body and called from
  outside while a binding it forwards to is still pending is unreachable
  under body ordering — so a later change to that ordering trips it.
- Slots, deps, wake and notify, the priority bands, the drain, and reinstall
  behave identically over both kinds; the existing slot-count and TCO
  assertions hold unchanged, and a reinstall inside a tree is on the Miri
  slate in its scheduler shape.

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
- *Slab width — open.* A `CellGraph`'s width is a constant of its type
  (`CellGraph<Work, 64>` names 4096 cells), and its two relations are inline,
  so a wide graph is `64 · W × W` words twice in its own bytes. This layer
  picks the `W` a worker runs at and how such a graph reaches the heap
  without being built on a worker stack first — `Box::new(CellGraph::new(..))`
  constructs in place only when the optimizer cooperates.
- *Per-edge push/pull default — open.* Push wherever the crossing rule
  applies, pull otherwise; or pull everywhere first and add push as an
  optimization. Recommended: push where the crossing rule applies, since
  that is the shape that keeps per-call cells out of the sealed tier.
- *Pressure model — open.* The substrate prices retention and reports
  occupancy of both tiers, and ships no threshold: the copy-versus-hold ramp
  — linear on occupancy, or a step at fixed watermarks — is chosen here, over
  the substrate's occupancy signal
  ([liveness-matrix.md § Bounding the two tiers](../../cellgraph/design/liveness-matrix.md#bounding-the-two-tiers)),
  from this embedder's own measurements. The prices reach this layer only
  through the crossing-verdict closure the table is constructed with
  ([cellgraph.md § The crossing
  verdict](../../cellgraph/design/cellgraph.md#the-crossing-verdict)); the
  ramp is that closure's body.

## Dependencies

**Requires:** none — the substrate it is rebuilt over is shipped.

**Unblocks:**

- [Publishing the workgraph crate](workgraph-extraction.md) — the published
  boundary is the layered pair.
