# The node table

**Problem.** The registry's node table is a hashbrown map in
[program storage](../../src/program/README.md). It doubles when full, and the
bump cannot take the old bucket array back. A
[type branch](type-branches.md) needs a table laid in a cell's region, but a
region's `Writer` only lays fixed runs and has no allocator. A hashbrown map
grows by asking an allocator for a new table, and implementing `Allocator` in
the lattice takes `unsafe`. A segmented chained table that grows only by
laying fixed runs was benchmarked against hashbrown over nodes that named
their children by digest: hashbrown was faster at hits, misses and dependent
lookups, and built about 3× faster when grown from empty. With
[linked type nodes](linked-type-nodes.md), the table serves only a handle's
entry into the lattice and interning, so it has not been measured on the work
it now does.

**Acceptance criteria.**

- A benchmark kept in the repo times build, hit, miss and entry lookups, and
  totals bytes per entry and dead bytes, over linked nodes for each candidate
  table: a hashbrown map, a safe open-addressing table that grows by laying a
  doubled run and rehashing into it, and the segmented chained table. Each is
  measured grown from its smallest size and presized.
- The benchmark times a two-tier lookup in each probe order: branch first,
  root first, and an empty branch skipped.
- The type lattice's design doc records the results and the table each tier
  uses.
- A branch's table grows only by laying fixed runs in its region.
- The type lattice adds no `unsafe`.

**Directions.**

- *The root's table — open.* A hashbrown map, which the program's bump can
  host, or the branches' table, so the two tiers share one structure.
- *The branches' table — open.* The safe open-addressing table or the
  segmented chained table. Hashbrown is a candidate only if a region's
  `Writer` gains an allocator.

## Dependencies

**Requires:**

- [Linked type nodes](linked-type-nodes.md) — the table is chosen on the
  workload of linked nodes.

**Unblocks:**

- [Type branches](type-branches.md) — a branch is laid as the chosen table in
  its region.
