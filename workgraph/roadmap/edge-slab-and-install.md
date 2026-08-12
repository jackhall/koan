# Edge slab and the install door

The expand half of edge-centric delivery: edges become first-class, koan-holdable
handles, alongside the existing `NodeId` surface.

**Problem.** The boundary currency is `NodeId`: koan holds node ids for parked
deps, dispatch placeholders, scope bindings, and run roots, and a dep edge is a
row in [dep_graph.rs](../src/scheduler/dep_graph.rs) keyed by those ids — not a
handle anyone owns. A `NodeId`'s validity depends on remote state (it is durable
only while the producer's slot still stands), so every koan read is preceded by
a probe and the misuse surface is non-local. The pinned design
([dag-scheduler.md § Edges and the boundary](../design/dag-scheduler.md#edges-and-the-boundary))
makes edges the sole boundary currency, self-owned and released by their owner —
but no edge slab exists in-tree.

**Acceptance criteria.**

- An edge slab exists mirroring `NodeStore`: `Vec<EdgeState>` plus free list,
  an `EdgeId` newtype, and debug-only generation stamps bumped at free.
- Each edge stores its producer `NodeId` (pre-fill, rewritable), a raw
  destination-region pointer, and a `cfg(debug_assertions)`-only weak shadow of
  the destination.
- An install verb wires an edge given a producer and a destination region and
  returns filled-or-parked
  ([dag-scheduler.md § Late wiring and install](../design/dag-scheduler.md#late-wiring-and-install));
  its signature takes the destination from day one, ahead of any deref.
- `RegionHandle::new(&Region)` mints the adopt capability crate-privately;
  `from_owner` remains the only embedder-side mint.
- The `NodeId`-keyed surface is untouched and koan compiles unchanged — this
  item is purely additive.
- The workgraph Miri slate (`cargo +nightly miri test -p workgraph --lib`)
  covers edge alloc/release recycling and install's filled and parked branches.

**Directions.**

- Destination coverage — open. Whether the wiring signature makes a non-covered
  destination unconstructible, or the wiring door checks the covered-by-owner
  condition at install. The containment lattice
  ([dag-scheduler.md § Edges and the boundary](../design/dag-scheduler.md#edges-and-the-boundary))
  is established here either way.
- Slab shape — decided per
  [dag-scheduler.md § Edges and the boundary](../design/dag-scheduler.md#edges-and-the-boundary):
  mirror `NodeStore`; a filled edge stores its resident terminal as an erased
  dormant `Retained` cell.
- Install's filled branch — decided. Pre-flip it reads the existing slot
  machinery internally; [delivery-at-finalize.md](delivery-at-finalize.md)
  rewires it to edge residents without touching call sites.

## Dependencies

**Requires:** none — foundation.

**Unblocks:**

- [Koan wires through edges](../../roadmap/refactor/edge-wiring-migration.md) —
  the migrate half adopts this surface.
