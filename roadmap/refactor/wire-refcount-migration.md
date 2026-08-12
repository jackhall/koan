# Adopt wire-refcounted scheduler retention

Koan's migrate step for
[wire-refcounted retention and reclamation](../../workgraph/roadmap/wire-refcount-retention.md),
per the crate-boundary protocol
([workgraph/roadmap/README.md](../../workgraph/roadmap/README.md#crossing-the-crate-boundary)).

**Problem.** The run loop drives the superseded verb set: `run_step` threads an
`owned_deps` list into `reclaim_deps`
([run_loop.rs](../../src/machine/execute/run_loop.rs)) although the wire
release needs no per-kind list, and no koan code releases root destinations —
the drain boundary
([interpret.rs](../../src/machine/execute/runtime/interpret.rs)) re-homes
consumer-less terminals but leaves entry slots unrooted, so top-level slots
rely on the accounting the workgraph expand deletes. Until this ships, koan
sits on whatever compatibility surface the expand left behind.

**Acceptance criteria.**

- `run_step` drops each consumer's wires through the scheduler's single wire
  release at step end; the `owned_deps` list plumbing is gone from the run
  loop.
- Submit paths take the run's root destination on each top-level entry slot,
  and the drain boundary releases them.
- The contract half: no superseded verb survives on workgraph's public surface —
  `reclaim_deps`'s owned-list form, `free`'s discharge triage, and any
  compatibility shim the expand kept for koan are deleted.
- `tools/verify.sh` passes, and the koan Miri slate covers a
  top-level-root release timeline at the drain boundary.

**Directions.**

- `Deps` currency — deferred. The owned/park labels stay as koan-side positional
  currency here; their collapse is
  [Collapse the Deps owned/park currency](deps-currency-collapse.md).

## Dependencies

**Requires:**

- [Wire-refcounted retention and reclamation](../../workgraph/roadmap/wire-refcount-retention.md)
  — the workgraph expand this migrates onto.

**Unblocks:**

- [Collapse the Deps owned/park currency](deps-currency-collapse.md) — once no
  scheduler semantics consume the split.
