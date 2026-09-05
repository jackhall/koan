# The cellgraph substrate

*(working name — `cellgraph` fixes a concept, not a final crate identifier)*

A graph of computation **cells**. A cell is one unit of suspended
computation: an identity, a region of safely allocated memory, an optional
continuation to run, and the holds that keep other cells alive on its behalf.
The graph makes no acyclicity promise about which cells reference which, has
no notion of a cell finishing, and lets a cell live indefinitely — re-entered,
held across arbitrary spans, or never entered at all. Liveness is decided by
the [liveness matrix](liveness-matrix.md). Everything that makes a
*scheduler* — dependency edges, wakeups, terminals, delivery — is layered
above by an embedder; `workgraph` is the first
([dag-scheduler.md](../../workgraph/design/dag-scheduler.md)). The dependency
direction is `koan` → `workgraph` → `cellgraph`, and each arrow is
compile-enforced: the lower crate names no type from the higher one.

## The cell

- **Identity** is a handle: slab slot plus generation, `Copy`. A handle names
  one occupant; an operation on a handle whose occupant has died is an
  error, never a silent no-op, because a stale handle means a caller kept a
  name past a death it declared.
- **Region.** One bump region per cell, in pointer-stable chunks, is the only
  place a value with reach may rest. Storage can leave the slot without a
  byte moving, which is what the sealed tier and absorption rely on.
- **Continuation**, optional. An erased, reattachable one-shot the substrate
  stores and hands back under `enter`, re-anchored at the step lifetime, and
  never calls. It rests beside the reach of whatever it captured, minted into
  its own cell's holds when it is stored — a cell holds what its continuation
  reads — which makes it the one stored mask the seal transition rewrites,
  and the read that hands it back the sealed tier's accessor. A step may
  store a successor before its scope ends. A cell with no continuation is
  **storage-only**, and is the substrate's answer to
  "a region that outlives its step but is never executed in": a cart a loop
  accumulates into, a mailbox a scheduler parks values in.
- **Holds**, in two relations. *Birth holds* fix the cell's parent chain at
  creation: a cell names at most one parent, and its birth row is the
  parent's row plus the parent's bit, derived by the substrate so transitive
  closure is by construction. *Pin holds* accrue as values with reach are
  minted into the region. Both are monotone for the cell's life, and both
  release wholesale rather than per reason: the birth row at the cell's
  declared death, the pin row when its slot leaves the slab — by clearing if
  the cell reclaims, by freezing into an aggregate if it seals.

## The contract: two embedder types

- **Continuation** — the work. A one-lifetime reattachable family
  (the erase-to-`'static` / re-anchor contract the witnessed core carries,
  [witnessed-memory.md](../../workgraph/design/witnessed-memory.md)).
  Everything an embedder knows about a cell that the substrate does not — its
  name-resolution state, its semantic frame, any output obligation — rides
  inside the continuation's captures, or as a value resident in the cell's
  region.
- **Value** — what passes between cells. A one-lifetime reattachable family
  carried witnessed: born in a region, carrying a reach mask, duplicated per
  reader, read only under a hold.

There is no frame type. Per-cell embedder structure is composed from cells: a
body cell and a storage-only cart cell, held together by ordinary holds, is
how an embedder gives one unit of work two regions with different lifetimes.

## Verbs

- **`create(parent?)`** hands back a handle, or refuses when the slab is at
  its cap. What to do on refusal is admission policy, and the embedder's.
- **`enter(handle, step)`** sets the cell's executing bit for the scope of
  `step` and supplies a step context. Within it a step can take the cell's
  continuation, re-anchored at the step lifetime and paired with the reach
  its captures read; allocate into its own region; allocate into any other
  live cell by handle (the destination-homed placement); mint a bare hold on
  another cell; read a carrier it built; and store a successor continuation,
  over captures or over nothing. That continuation read *is* the sealed
  tier's accessor — a capture whose region sealed since it was stored comes
  back named by sealed id, with reach derived from the record's aggregate —
  so there is no second door out of sealed storage. A cell cannot be entered
  while it is already executing.
- **`release(handle)`** declares death: the embedder promises never to enter
  the cell again. The slot leaves the slab once no descendant's birth row
  names the cell: reclaimed if nothing reaches its storage, sealed if
  something does.

The substrate never runs a cell, never chooses an order, and never inspects a
continuation. It names no thread model and no async runtime: it is a
single-mutator structure, and any concurrency is the embedder's arrangement
above it.

## Passing values between cells

There is no delivery protocol. A value is always resident in a live cell's
region, transient inside an executing cell's step, or sealed with its region.
Nothing else holds a value: there is no free-standing envelope with pins of
its own. Crossing a step boundary therefore takes one of two shapes, both
built from the verbs above:

- **Push.** While the producer executes, the value is minted into the
  consumer's region — or built there outright by destination-homed
  placement — and the consumer's row takes its mask. The producer can then
  die at column zero.
- **Pull.** The consumer mints a bare hold on the producer cell. The producer
  dies with a nonzero column and seals, and the consumer later reads through the
  sealed accessor inside its own step.

Which shape an edge takes is the embedder's choice, per edge.

## What is deliberately absent

Each absence is a design statement, not a gap:

- **No acyclicity of references.** Cells may reference each other freely.
  The *hold* graph must be acyclic for a cell ever to reclaim, but the
  substrate does not enforce it: a ring keeps both cells alive forever, which
  leaks rather than dangles. Preventing rings is the embedder's crossing
  discipline; the substrate ships a debug-mode ring detector, not a mint-time
  check.
- **No terminality, and therefore no error type.** Death is declared, not
  inferred, and carries no result. "Finished forever, with this value" — and
  the `Result` split between a witnessed value and a bare owned error — is a
  scheduler's terminal protocol.
- **No admission policy.** Creation refuses at the cap; a lazy generator of
  creations, a drain-first rule, or a hard failure is the layer above.
- **No frame payload.** Per-cell embedder data rides in the region, in
  captures, or in a companion cell.

## Open work

The remaining slices are indexed in [the roadmap](../roadmap/README.md):

- [Absorption](../roadmap/absorption.md)
- [Retention pricing](../roadmap/retention-pricing.md)
- [Rebuilding workgraph over cellgraph](../../workgraph/roadmap/adopt-cellgraph.md)
  — the first embedder's adoption.
