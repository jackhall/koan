# The workcell substrate

*(working name — `workcell` fixes a concept, not a final crate identifier)*

Beneath the DAG scheduler sits a smaller, more general library: a graph of
computation **cells**. A cell is one unit of suspended computation with three
ingredients — a continuation to run, safely allocated memory backing what the
continuation captures, and values that pass between cells. Nothing more: the
cell graph makes **no acyclicity guarantee**, has **no notion of a terminal**,
and a cell may be **long-lived** — re-entered, held across arbitrary spans,
or never finished at all. Everything that makes `workgraph` a *DAG scheduler*
— dependency edges, park/notify wakeups, cycle detection, terminal results,
delivery-driven retention, splicing — is layered on top of this substrate,
not part of it.

The dependency direction is `koan` → `workgraph` → `workcell`; each arrow is
compile-enforced (the lower crate names no type from the higher one).
[scheduler-library.md](scheduler-library.md) owns the overall division of
responsibility and the `workgraph` consumer API; this doc owns the cell
substrate's contract. [per-node-memory.md](per-node-memory.md) owns the
witnessed-memory mechanics the substrate is built from;
[witness-hosting.md](witness-hosting.md) owns reach-set representation and
the pinning invariant.

## The two halves

The substrate has a memory half and a cell half.

- **The witnessed memory substrate** — regions, brands, carriers, sealed and
  externally-witnessed cells, reach sets, the delivery envelope, and the step
  construction context. This half has no dependency on scheduling of any
  kind: it is the complete answer to "allocate values whose borrows are
  provably live, and move them between holders without a bare pin."
- **The cell table** — cells with identity, each holding an erased one-shot
  continuation witnessed by the cell's memory anchor. The table stores,
  hands back, and reclaims; it never inspects a continuation and never
  decides *when* a cell runs. Scheduling — queues, edges, wakeups — is the
  layer above.

## The cell contract

An embedder instantiates exactly three types. Each is stated from both
sides: what the embedder means by it, and the only things the substrate does
with it.

- **Continuation** — the work. A one-lifetime reattachable family
  (the erase-to-`'static` / re-anchor contract of
  [per-node-memory.md](per-node-memory.md)), stored erased and handed back
  once; the substrate re-anchors it witnessed by the cell's memory anchor
  and never calls it. Everything an embedder knows
  about a cell that the substrate does not — its name-resolution state, its
  semantic frame, any output obligation — rides *inside* the continuation's
  captures. A sealed carrier is a lifetime-free, self-pinning owned value,
  so a capture that needs a pin independent of the cell's own memory simply
  carries its own sealed cell.
- **Frame** — the memory anchor. The region owner whose `Rc` the substrate
  holds per cell so the continuation's captures stay live while the cell is
  dormant, and the witness under which the continuation is re-anchored. It
  is a `PinsRegion` region owner; the substrate retains and drops the `Rc`
  and calls nothing on it.
- **Value** — what passes between cells. A one-lifetime reattachable family
  carried as a witnessed/sealed carrier: born co-located with its reach set,
  duplicated per reader, re-anchored only under a pin. In-flight, a value
  travels as a delivery envelope — the sealed carrier paired with a retained
  frame owner — so no holder ever needs a bare pin.

## What is deliberately absent

Each absence is a design statement, not a gap:

- **No acyclicity.** Cells may reference each other in cycles; whether a
  reference topology must be acyclic is a property of a *scheduling
  discipline*, so the DAG layer owns cycle detection.
- **No terminality, and therefore no error type.** "This cell is finished
  forever, with this result" — including the `Result` split between a
  witnessed value and a bare owned error — is the DAG layer's terminal
  protocol. A cell substrate with long-lived cells cannot assume a cell ever
  produces a final answer.
- **No retention protocol.** Delivery-driven frame retention ("a producer's
  frame lives until every consumer has pulled") is defined in terms of dep
  edges and terminals, so it lives with them.
- **No payload, contract, or shell types.** An embedder detail that the
  substrate would only store and hand back is a continuation capture, not a
  contract type. Koan's lexical-position payload, its declared-return
  checker, and its per-call semantic shell all ride captures.

## workgraph above the substrate

`workgraph`'s embedder trait is the cell contract plus one addition: the
terminal **error** type its `Result`-shaped terminal protocol needs. On top
of the substrate it owns dependency edges (park and owned), notify lists and
work queues, cycle classification, terminal storage and delivery, retention
holds, and tail splicing. Koan instantiates the combined trait once
(`KoanWorkload`) and speaks only the consumer API described in
[scheduler-library.md](scheduler-library.md).

## Open work

- [Carving the workcell crate](../roadmap/scheduler_library/workcell-extraction.md)
  — the crate split itself.
- [Scheduler-owned frame storage](../roadmap/scheduler_library/scheduler-owned-frame-storage.md)
  — collapses the scheduler's per-cell memory anchors to the single `Frame`
  type this contract names.
- [Return contracts ride continuations](../roadmap/scheduler_library/contract-as-continuation.md)
  — dissolves the stored contract type into continuation captures.
