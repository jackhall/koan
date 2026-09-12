# Memory on cellgraph

Narrow `src/memory` to the shape of things in storage, over `cellgraph`'s
region and nothing else.

**Problem.** `memory` is koan's policy over `workgraph::witnessed`: a
reference-counted `FrameStorage` owner per call, a storage profile, a frame
shell, a frame-lifetime `RegionBrand`, and some twenty koan-bound aliases in
[substrate.rs](../../src/memory/substrate.rs). Every one of those names a
concept `cellgraph` does without — it has no frame type, identity is a handle,
liveness is a matrix, and a region is reachable only inside `enter`
([cellgraph/README.md](../../cellgraph/README.md)). [Values on
memory](values-on-memory.md) expected the swap to be an alias rewrite; it is a
narrowing. The old runtime behind `pending_rewrite` names the items that go,
and is not kept compiling: it is re-implemented layer by layer and dies as each
layer lands.

**Acceptance criteria.**

- `memory` depends on `cellgraph` and not on `workgraph`, `hashbrown` or
  `allocator_api2`. `substrate.rs` is a re-export block — the carrier states,
  `Writer`, the handles, the reattachable contract — binding the width `W`
  once and no koan parameter, since the library takes none.
- `region.rs` and `frame.rs` are deleted with their aliases: no `Rc`, no
  `FrameStorage`, no `RegionBrand`, no `KoanStorageProfile`, no run-root
  tier. A call's storage is a cell, and the resident is a value at rest in it.
- `program.rs` and `scope_id.rs` are unchanged; program storage stays a bump
  outside the graph.
- `SlotArray` is a run of `Cell<SlotState<V, P>>` laid down through
  `Writer::fill` at `'cell`, both parameters still the embedder's, its
  constructor taking a `Writer` and never a `StepContext`, the compile-time
  drop-free proof kept.
- Whether a keyed table shape survives is decided here and written into the
  README: an open-addressed probe table over `fill`, or none.
- `memory` names no item from the rest of koan, no continuation family, and
  its suite runs in the default slate.
- The `pending_rewrite` build is not a criterion. Old-runtime code that names
  a deleted item is left as is, and
  [TEST.md](../../TEST.md#the-pending-rewrite) says the feature build no
  longer compiles.
- [src/memory/README.md](../../src/memory/README.md) describes the narrowed
  module and links this item from `## Open work` until it ships.

**Directions.**

- *Gating the old aliases under `cfg(pending_rewrite)` — decided.* They are
  deleted, not gated: the old runtime is a requirements reference, not a build
  to preserve, and gating would leave a present-and-future mix in the module
  for the rewrite's duration.
- *Keyed shape — open.* Frames are slot-addressed once parse hands out
  layouts; modules and records may still want a keyed table. Decide against
  what [Scope on values and types](scope-on-values-and-types.md) needs.

## Dependencies

**Requires:**

- [Cell brand and writer doors](../../cellgraph/roadmap/cell-brand-and-writer.md) — the slot array is written through `fill` at `'cell`.

**Unblocks:**

- [Values on memory](values-on-memory.md) — a value is born through the narrowed module's doors.
