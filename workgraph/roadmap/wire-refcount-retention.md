# Wire-refcounted retention and reclamation

Replace the scheduler's split retention/reclamation accounting with one wire
kind refcounted from installation, per
[design/dag-scheduler.md](../design/dag-scheduler.md) (§ Push/notify dependency
edges, § Refcount reclamation, § Terminals and retention). Vocabulary is the
design doc's: a **wire** is the one dep-edge kind, a **standing destination** is
a wire's entry in its producer's refcount, the **wire release** is the one verb
that drops a consumer's wires, and a **root destination** is the run-held count
entry on a top-level entry slot.

**Problem.** The in-tree dep graph runs two parallel accounting systems that
disagree at their seams. Edges carry an `Owned`/`Notify` kind consumed only by
the free cascade. Retention pulls are seeded at `finalize` from the then-parked
consumer count, while a consumer that wires to an already-finalized producer is
counted through a separate channel (`DepGraph::owe_late_pull`). Discharge is
split across three verbs with different partial rules: `reclaim_deps`
discharges the late channel but not edges, `free` discharges both and
force-drops holds, and the free cascade kills owned children regardless of
other readers.

`alloc_node`'s ready-dep paths skip the accounting entirely (`alloc.rs:29–48`),
three holes in one function: a ready park dep is neither counted nor
discharged; a ready owned dep records a death-discharged edge with no matching
increment, tripping `decrement_pull`'s over-discharge `debug_assert`; and a
wire installed after a hold's release silently no-ops in `owe_late_pull` even
though `dep_delivered` `expect`s a live hold (`scheduler.rs:216`).

The skew has a structural cause: the same wiring semantics exist twice —
`alloc_node`'s inline loops, and the `add_owned_edge` / `add_park_edge` facades
behind `install_edges`.

**Acceptance criteria.**

- One scheduler-internal wire primitive installs every consumer→producer
  relationship; `alloc_node` and `install_edges` both route through it, and no
  wiring path branches on producer readiness for accounting (readiness gates
  only wake bookkeeping: notify entry and pending increment).
- Every wire adds one standing destination to its alias-resolved producer at
  installation; the wire release drops a consumer's wires exactly once, whether
  run at its end of step or at its death (Inv-B in
  [design/dag-scheduler.md](../design/dag-scheduler.md#the-dep-row-and-its-invariants)).
- The `DepEdge` `Owned`/`Notify` kind and the `owed` late-pull channel are gone
  from the dep graph; reclamation reads only the standing-destination count.
- A producer slot reclaims exactly when its count decrements to zero: hold
  released, anchor dropped, slot recycled, its own wires released recursively. A
  producer still counted by another destination survives any other consumer's
  death — no path force-kills a still-counted slot.
- Top-level entry slots carry a root destination the embedder releases at the
  drain boundary; a root terminal is readable until that release.
- A wired consumer's `dep_delivered` cannot observe a released hold, and a
  regression test covers the formerly-skipped path: wire at alloc to an
  already-finalized producer, then read.
- The workgraph Miri slate exercises the new release timelines: ready-at-wire
  read then release-at-zero, error-path death release, and owner death with a
  surviving reader (`cargo +nightly miri test -p workgraph --lib`).

**Directions.**

- Owner-death semantics — decided. Refcount-natural: a producer survives its
  spawner's death while any other destination stands; the forced `drop_retain`
  cascade is removed.
- Root destinations — decided. The run holds one destination per entry slot,
  released at the drain boundary; embedder-driven and id-based (no RAII guard,
  no global registry).
- Reclaim trigger — open. (a) Slots are born at count zero and reclaim only on
  a decrement-to-zero or an explicit release, so birth-zero never self-reclaims;
  (b) allocation confers a birth destination that wiring transfers to the
  consumer. Recommended: (a) — no transfer bookkeeping, and the root
  destination is just an ordinary standing destination.
- Where the count lives — open. On the `DepRow` from slot birth (the hold
  materializes under it at finalize), or inside the hold with a pre-finalize
  side count. Recommended: on the row — one counter, no merge step.
- Alias-slot counting — open. (a) Count wires at the resolved producer and pin
  alias rows separately; (b) count at the alias row itself, with the alias
  holding one forwarding wire to its producer. Recommended: (b) — aliases become
  ordinary refcounted nodes and reclaim with their readers, closing today's
  never-freed alias residual.
- Koan-facing surface — decided. This is the *expand* of the crate-boundary
  protocol ([README.md](README.md#crossing-the-crate-boundary)) and may break
  koan; the migrate item is
  [Adopt wire-refcounted scheduler retention](../../roadmap/refactor/wire-refcount-migration.md).

## Dependencies

**Requires:** none — foundation.

**Unblocks:**

- [Adopt wire-refcounted scheduler retention](../../roadmap/refactor/wire-refcount-migration.md)
  — koan's migrate step onto the new surface.
