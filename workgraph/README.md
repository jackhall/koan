# workgraph

A deferred-work DAG scheduler with a witnessed region-memory substrate
underneath it. An embedder schedules units of work with dependency edges
between them, allocates values into per-node regions, and passes
borrow-carrying results from producer to consumer — without writing `unsafe`
and without upholding any convention the compiler cannot check. Every
memory-safety invariant is enforced by a type (a brand, an opaque reach set, a
sealed carrier) or discharged inside the crate.

The crate names no type from its embedder: the dependency direction is
`koan` → `workgraph` → `cellgraph`, and each arrow is compile-enforced.
[koan](../README.md) is its first embedder.

## Doc tree

This directory carries the library's own docs, kept separate from koan's so the
crate reads as a standalone library rather than as one of koan's internals.

- [design/](design/) — the library's design docs.
  - [witnessed-memory.md](design/witnessed-memory.md) — the memory substrate:
    the erase-store / witness / reattach core, the `Region<P>` bump allocator,
    the `yoke` / `merge_pinned` / `map` construction surface with its
    one-wrapper-per-cell invariant, and the `seal` / `open` / `transfer_into`
    access surface.
  - [reach.md](design/reach.md) — reach evidence: the split into non-owning
    descriptions and holder-owned pin bundles, the three carrier states and
    their transform verbs, the holder rule, the mint rules (self, subsumption,
    eternal), and the delivery-driven retention model.
  - [dag-scheduler.md](design/dag-scheduler.md) — what the DAG layer adds over
    the cell substrate: the node-store lifecycle, push/notify dep edges and the
    dep-row invariants, the two-band work queue, alias splicing, and cascade
    reclamation.
  - [cellgraph.md](design/cellgraph.md) — the computation-cell substrate
    beneath the DAG layer (working name `cellgraph`): cells with a
    continuation, a memory anchor, and inter-cell values; no acyclicity, no
    terminality, long-lived cells.
  - [sectioned-reach.md](design/sectioned-reach.md) — reach evidence stored at
    sub-value granularity: the interned description side table and
    run-partitioned container storage.
- [roadmap/](roadmap/README.md) — open work on the library, and the
  expand / migrate / contract convention for a change that moves the boundary
  koan sits on.
- [observe/miri_slate.md](observe/miri_slate.md) — the Miri audit slate's
  run log.

Docs that state the *boundary* between the library and its embedder stay on
koan's side, because they describe the division rather than the library:
[design/scheduler-library.md](../design/scheduler-library.md) owns the
responsibility split and the consumer API, while
[design/per-node-memory.md](../design/per-node-memory.md) and
[design/witness-hosting.md](../design/witness-hosting.md) own koan's own
instantiation of the substrate above — which construction verb each koan site
takes, and koan's escape, residence and eternal-tier policy.

An embedder-facing walkthrough — workload instantiation, regions and carriers,
a minimal example embedder — is still owed, and lands with
[Publishing the workgraph crate](roadmap/workgraph-extraction.md).

## Verify

The crate has its own verification slate, so a library change can land ahead of
koan's adoption of it:

```sh
tools/verify.sh   # from the repo root; picks the library slate when every
                  # changed path is under workgraph/
```

That slate runs `cargo test -p workgraph --features test-hooks` (unit tests and
doctests, including the `compile_fail` escape guards), clippy on the same, and
the doc-link check. It reports whether koan still compiles as information, never
as a gate.
