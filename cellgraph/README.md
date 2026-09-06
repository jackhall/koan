# cellgraph

*(working name)*

A substrate of computation **cells**: each one an identity, a bump region of
safely allocated memory, an optional erased continuation, and the holds that
keep other cells alive on its behalf. Liveness is an attributed bit matrix
over a bounded slab plus an atomic sealed tier, so a cell is reclaimed the
instant no bit names it — no count gates a slab slot, and the sealed tier's
holder count is released only wholesale, at a holder's own death. The
substrate makes no acyclicity promise, has no notion of a cell finishing, and
never decides when a cell runs — a scheduler is something an embedder builds
on top.

The crate names no type from its embedders: the dependency direction is
`koan` → `workgraph` → `cellgraph`, and each arrow is compile-enforced.
[workgraph](../workgraph/README.md) is the first embedder, and
[koan](../README.md) sits above that.

The crate is written off to one side of `workgraph`, which is rebuilt over it
by [adopt-cellgraph.md](../workgraph/roadmap/adopt-cellgraph.md).

## Source layout

- [src/lib.rs](src/lib.rs) — the module wiring and the public surface.
- [src/handle.rs](src/handle.rs) — cell identity: slot plus generation, and
  the stale-handle refusal.
- [src/table.rs](src/table.rs) — the slab, the `create` / `enter` / `release`
  verbs, the step context's doors, the seal transition, the three locality
  merges a dying cell can take instead, the cascade that retires cells and
  records, and the read-only price queries. The price queries are internal:
  what retention costs is a number the substrate computes, not a door an
  embedder opens, and it reaches the embedder through the crossing verdict of
  [resident-carriers.md](roadmap/resident-carriers.md).
- [src/matrix.rs](src/matrix.rs) — `Bits`, the crate's one row of bits and the
  only place word-and-bit arithmetic is written, and the flat birth and pin
  matrices built over it.
- [src/mask.rs](src/mask.rs) — reach as a hybrid mask: a `Bits` row over slab
  slots plus a sparse sealed-id set.
- [src/sealed.rs](src/sealed.rs) — the sealed tier: ids, sparse sets, frozen
  aggregates, holder counts, detached storage.
- [src/region.rs](src/region.rs) — the per-cell bundle of bumps, the splice a
  locality merge performs, and the write surface a build closure receives.
- [src/carrier.rs](src/carrier.rs) — the two carrier states a built value
  passes through: sealed with its reach, and opened at a reading borrow.
- [src/reattach.rs](src/reattach.rs) — the reattachable contract and the
  single lifetime-retype the crate is built on.
- [tests/surface.rs](tests/surface.rs) — the public surface, named and
  exercised from outside the crate. Everything an embedder may reach is used
  here and nothing else is reachable to use, so an item that widens shows up
  as an unused import and an item that goes missing as a compile error.
  [tools/verify.sh](../tools/verify.sh) runs it under `--release` as well, since
  a surface that changed shape with the build profile would compile for an
  embedder in one profile and not the other.

Memory-safety sign-off for the retype seam is
[observe/miri_slate.md](observe/miri_slate.md).

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
