# cellgraph

*(working name)*

A substrate of computation **cells**: each one an identity, a bump region of
safely allocated memory, an optional erased continuation, and the holds that
keep other cells alive on its behalf. Liveness is an attributed bit matrix
over a bounded slab plus an atomic sealed tier, so a cell is reclaimed the
instant nothing names it and never lingers behind a count. The substrate
makes no acyclicity promise, has no notion of a cell finishing, and never
decides when a cell runs — a scheduler is something an embedder builds on top.

The crate names no type from its embedders: the dependency direction is
`koan` → `workgraph` → `cellgraph`, and each arrow is compile-enforced.
[workgraph](../workgraph/README.md) is the first embedder, and
[koan](../README.md) sits above that.

The crate is being written off to one side: this tree currently holds its
design and roadmap, and the code lands slice by slice per the roadmap before
`workgraph` is rebuilt over it.

## Doc tree

- [design/](design/) — the substrate's design.
  - [cellgraph.md](design/cellgraph.md) — the cell: identity, region,
    continuation, holds; the two embedder types; the `create` / `enter` /
    `release` verbs; push and pull as the two ways a value crosses cells;
    what is deliberately absent.
  - [liveness-matrix.md](design/liveness-matrix.md) — liveness as attributed
    bit matrices over the slab: pin holds and birth holds, reach as a hybrid
    mask, the sealed tier and its accessor, the seal transition, the
    invariants and the staleness argument, absorption, pricing.
- [roadmap/](roadmap/README.md) — the slices that build the crate, in
  dependency order.

Docs that state the *boundary* between the substrate and its embedders stay
with the embedder: [dag-scheduler.md](../workgraph/design/dag-scheduler.md)
owns what `workgraph` adds above the cell, and
[scheduler-library.md](../design/scheduler-library.md) owns koan's side of the
stack.
