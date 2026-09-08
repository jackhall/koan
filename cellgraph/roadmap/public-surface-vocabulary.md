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
- The hold graph's internal node type is `GraphNode`, and both node enums name
  the tier rather than the kind: `GraphNode::Slab` / `GraphNode::Sealed` and
  `HoldNode::Slab` / `HoldNode::Sealed`, matching `CellHandle::Slab` and
  `SlabForward::Slab`. A `Cell` variant would read as a contrast with `Sealed`
  on the wrong axis — a sealed cell sits in the other tier, it is not un-live —
  and `Live` would be plainly false, since `disposable` gates on birth holds
  alone, so a released cell a pin row still names stays `Dead` in the slab and
  the walk descends into it. `HoldNode` keeps its type name: the level it names
  against `GraphNode` is "by handle" versus "by slot", and that pairing stays
  open.
- `tools/verify.sh` is green and `tools/doclinks.py check` reports no broken
  link, design-tree anchors included.

## Dependencies

**Requires:** none.

**Unblocks:** none tracked yet.
