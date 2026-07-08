# Carving the workcell crate

**Problem.** `workgraph` entangles two layers
([design/workcell.md](../../design/workcell.md) states the target split).
The witnessed memory substrate
([witnessed.rs](../../workgraph/src/witnessed.rs) and its submodules)
already has no dependency on scheduling, but the cell half — nodes holding
erased continuations witnessed by memory anchors — lives inside
[node_store.rs](../../workgraph/src/scheduler/node_store.rs)'s slot table,
interleaved with DAG-only state: `SlotState` terminality, dep edges,
notify/park bookkeeping, retention holds, splice aliases. There is no crate
an embedder can take that offers "cells with continuations, safe memory, and
inter-cell values" without also taking acyclicity, terminal `Result`
semantics, and the retention protocol.

**Acceptance criteria.**

- A `workcell` (working name) workspace crate exists; `workgraph` depends on
  it and it depends on neither `workgraph` nor `koan` (the dependency
  direction is compile-enforced).
- Its cell contract names exactly three embedder types — continuation,
  frame (memory anchor), value — and its cell table makes no acyclicity or
  terminality assumption: a cell may be long-lived and cells may reference
  cyclically.
- The witnessed memory substrate (regions, brands, carriers, reach sets, the
  delivery envelope, the step construction context) ships in `workcell`.
- `workgraph`'s `Workload` is the cell contract plus the terminal error
  type; dep edges, park/notify, cycle detection, terminal storage, retention
  holds, and splicing appear only in `workgraph`.

**Directions.**

- *Crate name — open.* `workcell` is a working name; the final identifier is
  settled with [workgraph-extraction.md](workgraph-extraction.md)'s naming
  pass.
- *Slot-table split — open.* (a) `workgraph` wraps `workcell`'s cell table
  (composition: DAG state in a parallel table keyed by cell id); (b) the
  cell table is parameterized over an extension slot `workgraph` fills.
  Recommended: (a) — composition keeps the cell table's surface free of DAG
  vocabulary.

## Dependencies

**Requires:**

- [Scheduler-owned frame storage](scheduler-owned-frame-storage.md) — the
  trait's memory types must already be down to the single frame anchor.
- [Return contracts ride continuations](contract-as-continuation.md) — the
  cell contract has no contract type to carve around.

**Unblocks:**

- [Publishing the workgraph crate](workgraph-extraction.md) — the published
  boundary is the layered pair.
