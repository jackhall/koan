# Settle the public surface's vocabulary

Rename the crate's exported nouns to the ones its own design docs use — the
hold graph, the slab, and the three states a value with reach passes through —
so an embedder taking the crate up meets one vocabulary rather than two.

**Problem.** The public surface is named from the inside out. Each of these is
a name a reader has to translate before it means anything:

- **`CellTable`** is the hold graph ([design/cellgraph.md](../design/cellgraph.md)),
  and "table" is spent twice more inside the crate — on a cell's reach table
  and on the scratch region. The container is the one thing that is *not* a
  table.
- **`Handle` / `Slot` / `SlotState`** name the slab habitat without saying so,
  while `TreeHandle` / `TreeSlot` / `TreeState` name the other one explicitly.
  The unqualified half reads as the general case; it is not.
- **`Dormant` and `Resident`** are near-synonyms sitting on the two states
  that must never be confused. `Dormant` is the in-step carrier, which holds a
  lifetime, a live value, and an entitlement to its storage — nothing about it
  is dormant. `Resident` is the at-rest form: parked `MaybeUninit` bytes whose
  home may already be gone, which is the deeper sleep of the two.

**Acceptance criteria.**

- `CellTable` is `CellGraph`; `src/table.rs` is `src/graph.rs` and `src/table/`
  is `src/graph/`. "Table" survives in the crate only for the reach table.
- The slab habitat is named on every type that belongs to it: `Handle` is
  `SlabHandle`, `Slot<C, W>` is `SlabCell`, `SlotState` is `SlabState`. Bare
  `Slot` no longer names a type. `TreeSlot` becomes `TreeCell` in the same
  pass, so the two habitats read as a pair.
- The three carrier states are named in liveness order — **`Dormant` >
  `Ready` > `Active`**. Today's `Resident` becomes `Dormant`, today's
  `Dormant` becomes `Ready`, and `Active` is unchanged. `ResidentKey` follows
  to `DormantKey` and `src/resident.rs` to the module the new name wants.
- No prose still calls the in-step carrier dormant, and no prose uses
  "resident" for a carrier at all. The unrelated slot sense — `SlotState::Dead`
  documented as "the resident state", and `Occupancy::occupied`'s
  "dead-but-resident" — is reworded rather than left as the word's only
  survivor.
- `SlabForward::Slab { base }` is `first_index`: it is the offset the absorbed
  reaches start at, and `base` says none of that.
- `src/mask.rs` is `src/reach.rs`. The module is named for a word its only
  type stopped using when `GraphReach` took its name.
- `tools/verify.sh` is green and `tools/doclinks.py check` reports no broken
  link, design-tree anchors included.

**Directions.**

- *`HoldNode` vs `SlotNode` — open, and not ready.* One hold-graph node type at
  two levels: `SlotNode::Cell(u32)` is the internal one the walkers run on
  (`walk`, `walk_for_ring`, `holds_of`, `transitive_pins`, `prime_memo` — 33
  sites), and `HoldNode::Cell(Handle)` is the same node with its generation
  attached, minted by `CellGraph::name` for the single `#[cfg(test)]` caller
  `debug_ring_from`. `Hold` vs `Slot` names neither level, so the pair reads as
  two *kinds* of node. Candidates considered and not settled: `HoldNode` /
  `HoldSlot`, `NamedNode` / `HoldNode`, `HoldNode` / `HoldNodeBySlot`.

## Dependencies

**Requires:** none.

**Unblocks:** none tracked yet.
