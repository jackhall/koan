# Sectioned reach

Implement [workgraph/design/sectioned-reach.md](../../workgraph/design/sectioned-reach.md):
the interned description side table, the run-partitioned container storage,
and the alloc door — workgraph-side only, with tests; koan adoption is
[Sectioned substrates](../untyped_arena/sectioned-substrates.md).

**Problem.** Reach evidence is stored only per whole value. The region's
description side table
([`Region::alloc_reach`](../../workgraph/src/witnessed/region.rs)) appends
unconditionally — minting an already-seen member set allocates a duplicate
description and re-folds its owning pins into the region's union via
`Region::retain_reach` — and workgraph offers no container storage that
records reach at sub-value granularity, so an embedder that parts a cell
from its container has nothing stored to read and must re-derive reach by
walking the value.

**Acceptance criteria.**

- The region's description side table is an intern table:
  [`ReachDescription::mint`](../../workgraph/src/witnessed/reach.rs)
  get-or-mints keyed on the canonical member set (member owner addresses,
  sorted), returning the existing co-located entry on a hit and allocating
  only on a miss; the `(&'a description, PinBundle)` caller contract is
  unchanged.
- On an intern hit the `Region::retain_reach` fold is skipped — one
  description and one region-lifetime pin fold per distinct reach per
  region.
- The empty description is a per-region interned singleton shared by every
  region-pure value and every owned-data run.
- `workgraph` exports a payload-generic sectioned container: cells in
  semantic order, physically partitioned into contiguous runs that each
  pair a span of cells with one interned `&ReachDescription`; the run
  covering an index resolves by binary search over run starts, and a
  single-run container carries one description with no per-cell cost.
- A run's description is exactly its cells' shared reach (adjacency
  decides sharing), and a projected cell is `'a`-confined to its
  container's region through both the payload and the run's description
  reference — outliving the container without a mint-consuming relocation
  seam is a compile error.
- Sectioned containers are built through one alloc door — constructor plus
  per-input `(payload, envelope, copy-or-pin verdict)`: a fully-owned
  copied input lands in an empty-reach run with no walk; a pinned input's
  run description get-or-mints from its members plus its home region, its
  owning pins folding into the destination's union bundle (skipped on a
  hit); the container's value-level description is the get-or-mint union
  over its run descriptions, so whole-value carriers keep their single
  stored description shape.
- Workgraph-side tests cover interning (miss allocates, hit returns the
  existing entry, hit skips the retain fold, the empty singleton),
  sectioning (adjacency grouping, same reach in non-adjacent runs, the
  degenerate length-one interleaving, index lookup, the single-run fast
  path), and the alloc door (copied, pinned, and fully-owned inputs; the
  value-level union) — with no koan type in scope.
- The Miri audit slate is green.

**Directions.**

- *Phasing — open.* (a) Interning ships first as its own PR — the mint
  contract is unchanged, and the dedupe is independently valuable — then
  sectioned storage and the alloc door; (b) the whole workgraph side in
  one PR. Recommended: (a).
- *Intern lookup structure — open.* The append arena has no lookup, so the
  intern needs its own index: (a) a map from canonical key to table entry
  alongside the arena; (b) a linear scan over the table, betting per-region
  tables stay small.

## Dependencies

**Requires:** none — ready to start.

**Unblocks:**

- [Sectioned substrates](../untyped_arena/sectioned-substrates.md) — the
  koan adoption routes substrates through this machinery.
- [Carving the workcell crate](workcell-extraction.md) — landing first
  keeps the carved memory-substrate surface final.
