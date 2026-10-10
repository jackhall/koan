# The node table

**Problem.** The registry's node table is a hashbrown table of references to
stored nodes, in [program storage](../../src/program/README.md). It doubles
when full, and the bump cannot take the old bucket array back. A
[type branch](type-branches.md) needs a table laid in a cell's region, but a
region's `Writer` only lays fixed runs and has no allocator. A hashbrown map
grows by asking an allocator for a new table, and implementing `Allocator` in
the lattice takes `unsafe`. A node names each child by reference, so the table
serves only a handle's entry into the lattice and interning. Over that work, a
safe open-addressing table (16 B slots holding a fingerprint and a reference,
linear probing, grown by laying a doubled run) was measured against hashbrown
on linked nodes, grown from empty and presized, at 1,000 to 60,000 entries and
in branches of 1 to 8:

| | open addressing | hashbrown |
|---|---|---|
| dependent hit, warm, 60k | 25 ns | 26 ns |
| dependent hit, cold, 60k | 240 ns | 324 ns |
| miss, 60k | 6–9 ns | 1–3 ns |
| hit in a branch of 8 | 2.5 ns | 6.1 ns |
| table bytes per entry, grown | 33–44 B | 12–20 B |
| build grown over presized | 2–6× | 2–3× |

Warm hits tie because hashbrown's control bytes stay in L2 at this size; cold,
the open table saves one DRAM line. Hashbrown's miss is one well-predicted SIMD
group probe where linear probing takes a mispredicted branch per slot. In two
tiers a root miss is structural, paid by every branch handle that enters root
first, so the root's miss cost sets the probe order.

**Acceptance criteria.**

- The benchmark is kept as a `cargo bench` target of the lattice crate. It
  times build, hit, miss and entry lookups, and totals bytes per entry and
  dead bytes, over linked nodes for a hashbrown map and the open-addressing
  table, each grown from its smallest size and presized, and times a two-tier
  lookup in each probe order: branch first, root first, and an empty branch
  skipped.
- The type lattice's design doc records the results and the table each tier
  uses.
- A branch's table grows only by laying fixed runs in its region.
- The type lattice adds no `unsafe`.

**Directions.**

- *The root's table — decided.* Hashbrown, which the program's bump hosts.
  Its misses are 5× cheaper than the open table's, which makes a root-first
  probe nearly free for a branch handle, and the root is built during the
  load, where its 2–3× grown build and one stranded bucket array are paid
  once.
- *The branches' table — decided.* The safe open-addressing table, grown by
  laying a doubled run in the region and rehashing into it. A branch hit
  costs 2.5 ns against hashbrown's 6, and a region's `Writer` can lay it.
  The segmented chained table lost on hits, misses and build when measured
  earlier, and its one advantage, fewer dead bytes, is what
  [presizing](branch-presizing.md) takes.
- *What ships — decided.* The branches' table as a public lattice type,
  generic over an entry that carries its own 128-bit key, so the benchmark
  times the shipped table over stand-in nodes. Nothing in the lattice lays one
  until [type branches](type-branches.md) does.
- *How a branch's table grows — decided.* Through a one-method trait that lays
  a fixed run of drop-free values, implemented for a bump in the lattice. An
  insert takes the layer as an argument, so the table stores no writer. Koan
  implements the trait for a newtype over a region's `Writer`, since neither
  crate names the other.
- *Fingerprint slots or bare references — deferred* to
  [presizing a branch](branch-presizing.md), whose measurement counts a
  branch's traffic. The table ships with 16 B fingerprint slots.

## Dependencies

**Requires:** none.

**Unblocks:**

- [Type branches](type-branches.md) — a branch is laid as the chosen table in
  its region.
