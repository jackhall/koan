# Presizing a branch

**Problem.** A [type branch](type-branches.md) starts at its smallest size
and doubles as the run interns into it, laying each doubled run in its region.
The region cannot take a superseded run back, so a grown branch strands every
run but its last: with 16 B slots a 256-entry branch strands 8 KiB against
12 KiB of nodes, and laying and filling the fresh runs is half the branch's
build time. A branch laid at the size it will reach strands nothing and builds
in a third of the time. Which size that is cannot be read from a body's text
alone, since a loop over varying content interns per distinct content, but a
body's doors and its past activations both say something about it.

**Acceptance criteria.**

- A measurement over the example programs of how well each predictor below
  sizes a branch: the interns of run-built content per activation, keyed by
  the function, against the size the predictor would have laid. It also
  counts each branch's hits, misses and probes past an occupied slot.
- A branch is laid at its predicted size when a prediction exists, and still
  grows past it.
- The share of a region's branch bytes stranded by growth is reported by the
  small-branch dead-share column of the lattice's
  [node-table benchmark](../../lattice/benches/node_table.rs) and by the
  measurement above.

**Directions.**

- *A static count — open.* The load counts a body's interning doors, the view
  doors, functor runs, run-built code shapes and per-call nominals, and the
  nodes one pass through the body builds, and lays the branch at that size. A
  loop over varying content exceeds it, so growth remains.
- *A shape-record memo — open.* The record beside a body shape remembers the
  size its last activation's branch reached, and the next activation lays that
  size; a shape branch keeps its high-water mark there across resets. The memo
  is a hint: a wrong one costs one stranded run, as today.
- *A recycled-region memo — open.* A reset region remembers its previous
  branch's size, suited to tail hops where the same body returns.
- *Fingerprint slots or bare references — open.* A branch's table holds 16 B
  slots, the digest's low word beside the reference, so a probe past an
  occupied slot reads no node. An 8 B slot halves the table's bytes, and with
  them what growth strands, but reads a node line on every such probe, about
  one hit in two at half load. Warm, the two tie within 2 ns; cold, the bare
  slot pays 25 ns more per hit. Settled by the branch traffic the measurement
  above counts.
- *A root-only site hint — open.* A site whose static type is exact and closed
  carries that very type as its memo, which the load interned, so a read of
  the memo there can skip the branch probe; a type value's payload is a handle
  of its own. Any other site can hold a memo the run builds, even over root
  types alone: a list's memo is the join of its cells, so `[p]` over
  `p :(Number | Str)` builds `LIST OF Number`. With a hashbrown root, the root
  miss a branch handle pays costs under half a nanosecond, so the hint buys
  little until the branch probe itself costs more than that.

## Dependencies

**Requires:**

- [Type branches](type-branches.md) — the branch whose size is predicted.

**Unblocks:** none.
