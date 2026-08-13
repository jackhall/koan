# Koan wires through edges

The migrate half of edge-centric delivery's boundary currency: koan holds
`EdgeId`s and wires through the install door, with delivery timing unchanged.

**Problem.** Koan names `NodeId`s across the dispatch and runtime layers and
classifies producers by probing scheduler state before wiring: the
ready/errored/park ladder in
[dispatch.rs](../../src/machine/execute/dispatch.rs), the `is_result_ready`
probes before park/forward in
[runtime.rs](../../src/machine/execute/runtime.rs), and drain-boundary root
reads by id in [interpret.rs](../../src/machine/execute/runtime/interpret.rs).
Each probe re-derives what the wiring verb could have answered, and each held
`NodeId` leans on the remote-validity discipline the pinned design retires
([dag-scheduler.md § Edges and the boundary](../../workgraph/design/dag-scheduler.md#edges-and-the-boundary),
koan-side split in
[scheduler-library.md](../../design/scheduler-library.md)).

**Acceptance criteria.**

- Koan holds `EdgeId`s — not `NodeId`s — for parked deps, dispatch
  placeholders, scope bindings, and the run's roots; all wiring goes through
  the install verb.
- Producer classification is install-and-inspect: dispatch and the
  runtime park/forward sites act on install's filled-or-parked return, and the
  standalone probe doors (`is_result_ready`, the producer-standing/disposition
  classification) are deleted from workgraph's surface (the contract half of
  this item).
- A placeholder park's edge names the original destination scope's region.
- Drain-boundary root reads go through koan-held root edges, released by koan
  before frame teardown.

**Directions.**

- `NodeId` internalization timing — decided. The step-start dep pull stays
  `NodeId`-keyed inside the run loop's bookkeeping and is not rewritten
  edge-keyed here — that transitional pull would be deleted wholesale by the
  delivery flip, so full internalization is
  [delivery-at-finalize.md](../../workgraph/roadmap/delivery-at-finalize.md)'s
  criterion.

## Dependencies

**Requires:**


**Unblocks:**

- [Delivery at finalize](../../workgraph/roadmap/delivery-at-finalize.md) — the
  flip lands deep-and-narrow only once koan already speaks edges.
