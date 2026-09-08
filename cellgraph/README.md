# cellgraph

*(working name)*

A substrate of computation **cells**: each one an identity, a bump region of
safely allocated memory, an optional erased continuation, and the holds that
keep other cells alive on its behalf. Liveness is an attributed bit matrix
over a bounded slab plus an atomic sealed tier, so a cell is reclaimed the
instant no bit names it — no count gates a slab slot, and the sealed tier's
holder count is released only wholesale, at a holder's own death. A third
habitat sits outside the matrix entirely: a **tree cell**
([design/tree-cells.md](design/tree-cells.md)) takes no slab slot and no bit,
because a call subtree's liveness is a stack discipline — a parent outlives
its children — so a recursion deeper than the slab's cap costs the pool a slot
and the matrix nothing. The substrate makes no acyclicity promise, has no
notion of a cell finishing, and never decides when a cell runs — a scheduler
is something an embedder builds on top.

The crate names no type from its embedders: the dependency direction is
`koan` → `workgraph` → `cellgraph`, and each arrow is compile-enforced.
[workgraph](../workgraph/README.md) is the first embedder, and
[koan](../README.md) sits above that.

The crate is written off to one side of `workgraph`, which is rebuilt over it
by [adopt-cellgraph.md](../workgraph/roadmap/adopt-cellgraph.md).

## Source layout

- [src/lib.rs](src/lib.rs) — the module wiring and the public surface.
- [src/handle.rs](src/handle.rs) — cell identity over both habitats: a slab
  slot or a tree-pool index paired with a generation, `CellHandle` naming
  either kind, and the stale refusals.
- [src/table.rs](src/table.rs) — the slab, the `create` / `enter` / `release`
  verbs, the step context's doors, the seal transition, the three locality
  merges a dying cell can take instead, the cascade that retires cells and
  sealed cells, the relocation map that forwards a resident through a merge,
  and the read-only price queries. The price queries are internal: what
  retention costs is a number the substrate computes, not a door an embedder
  opens, and it reaches the embedder through the crossing verdict
  ([cellgraph.md § The crossing
  verdict](design/cellgraph.md#the-crossing-verdict)).
- [src/tree.rs](src/tree.rs) — the tree pool: the slab-free habitat for a
  call subtree, whose liveness is a stack discipline rather than a matrix
  reading. The chain links and depth the ancestry rule classifies by, the
  child count that keeps a released parent resident, the pledge a placement
  door leaves that says which ancestor a dying cell's bump splices into, and
  the tombstone chain a resident redeems through once its home's bytes have
  moved ([design/tree-cells.md](design/tree-cells.md)).
- [src/matrix.rs](src/matrix.rs) — `Bits`, the crate's one row of bits: `W`
  words held inline, `Copy`, and the only place word-and-bit arithmetic is
  written outside the two loops that keep a matrix's tally in step with its
  rows. The birth and pin matrices are inline arrays of those rows, so a table
  carries both relations in its own bytes.
- [src/mask.rs](src/mask.rs) — reach as a hybrid mask: an inline `Bits` row
  over slab slots plus a sparse sealed-id set that is itself inline up to two
  ids, so a reach naming at most two sealed regions is built, copied, and
  compared without touching the allocator.
- [src/sealed.rs](src/sealed.rs) — the sealed tier: ids packed as a serial
  beside a slab index, sparse sets, frozen aggregates, holder counts, and the
  dense slab of sealed cells with its free list.
- [src/region.rs](src/region.rs) — the per-cell bundle of bumps, the splice a
  locality merge performs, the write surface a build closure receives, and the
  sealed cell's frozen-closure memo, kept in the region's own bytes.
- [src/scratch.rs](src/scratch.rs) — the table's one scratch region: a bump
  sized at construction, reset at the entry of every verb and never inside
  one, and the doors every transient a verb builds is taken through — the
  growable worklists a cascade nests, the exactly-sized runs a placement
  builds per operand, and the views a build closure receives. Nothing with
  drop glue may go in it, since a reset runs no destructor, and each door
  asserts that at compile time.
- [src/carrier.rs](src/carrier.rs) — the two carrier states that carry a
  lifetime: sealed with its reach, in step, and opened at a reading borrow.
- [src/dormant.rs](src/dormant.rs) — the third carrier state, at rest: a
  value parked between steps with no lifetime of its own, the private key
  naming its reach, and the per-cell resident table that reach lives in —
  interned on content, so the table is bounded by the distinct reaches a cell
  has been kept into rather than by how many times.
- [src/reattach.rs](src/reattach.rs) — the reattachable contract and the
  single lifetime-retype the crate is built on.
- [tests/surface.rs](tests/surface.rs) — the public surface, named and
  exercised from outside the crate. Everything an embedder may reach is used
  here and nothing else is reachable to use, so an item that widens shows up
  as an unused import and an item that goes missing as a compile error.
  [tools/verify.sh](../tools/verify.sh) runs it under `--release` as well, since
  a surface that changed shape with the build profile would compile for an
  embedder in one profile and not the other.

Memory-safety sign-off for the retype seam is
[observe/miri_slate.md](observe/miri_slate.md).

## Measuring

What a verb costs is measured per verb: the allocations one `create`, `enter`,
`alloc_into` or `release` made, the bytes it asked for, and how long it took —
each exclusive of the doors it ran inside it, so `enter` reports the step
machinery and not the `alloc` within it.

- [perf/](perf/) — the harness, a `[[bin]]` behind the `perf` cargo feature so
  the library build, its tests, and the Miri slate never compile it.
  [perf/shapes.rs](perf/shapes.rs) holds the shapes — a keep-and-redeem loop, a
  push chain, a pull chain, a birth chain, a fan-out placement, a shared
  sub-tier wound down, a cell kept into at many distinct reaches, and a chain
  of tree cells each pinning its result into its parent — and [perf/meter.rs](perf/meter.rs) the meter, which
  subtracts a nested door's spend from its parent's frame. It counts through
  [audit/counting_alloc.rs](../audit/counting_alloc.rs), the same delegating
  allocator koan's own readings go through.
- [tools/cellgraph_perf.py](../tools/cellgraph_perf.py) — the one command.
  `python3 tools/cellgraph_perf.py` sweeps the set and prints a delta against
  the newest recorded commit, which it rebuilds and runs beside HEAD so the
  time column is a comparison taken in one sitting rather than a figure written
  down in another. `--record` appends HEAD's readings, `--gate` exits non-zero
  if allocations or bytes rose, and `--gate-time` exits non-zero if any row's
  fastest trial sits more than 10 % above the rebuilt baseline's, printing the
  bar each row was held to. Every row is weighed: the harness runs each shape
  in blocks sized so the smallest row's block clears 20 µs and reports the
  fastest block per run, and the tool execs every trial from a fresh copy of
  its binary so neither side reads from one fixed placement of its pages. The
  tolerance comes from `--calibrate`, which sweeps HEAD against a rebuild of
  HEAD — identical source, so every row's movement is this machine's own
  spread — and prints what the tolerance has to clear.
- [observe/perf.csv](observe/perf.csv) — the record: a tidy dataframe, one row
  per `(date, sha, dirty, benchmark, n, cap, verb)` carrying `calls`,
  `allocations`, `bytes` and `nanos`, capped to the three most recently
  recorded commits and read with `pandas.read_csv` and a pivot. Allocations and
  bytes are deterministic and gate a change on their own; a recorded `nanos`
  is there for the trend and is never asserted, since it was read in another
  session on a machine doing other things — wall time is held to a bar only
  under `--gate-time`, and only against a baseline rebuilt and run beside the
  sweep.

## Doc tree

- [design/](design/) — the substrate's design.
  - [cellgraph.md](design/cellgraph.md) — the cell: identity, region,
    continuation, holds; the two embedder types; the `create` / `enter` /
    `release` verbs; push and pull as the two ways a value crosses cells;
    what is deliberately absent.
  - [liveness-matrix.md](design/liveness-matrix.md) — liveness as attributed
    bit matrices over the slab: pin holds and birth holds, reach as a hybrid
    mask, the sealed tier and its accessor, the seal transition, the
    invariants and the staleness argument, absorption, pricing.
  - [tree-cells.md](design/tree-cells.md) — the third region habitat: a cell
    of a call subtree, living under a slab root in an uncapped pool that no
    mask names. Mints redirected to the root, the ancestry rule an
    operand homed in a tree cell meets, the pledge and the splice that
    replace a seal at its death, and the tombstone chain a redeem follows.
- [roadmap/](roadmap/README.md) — the slices that build the crate, in
  dependency order.

Docs that state the *boundary* between the substrate and its embedders stay
with the embedder: [dag-scheduler.md](../workgraph/design/dag-scheduler.md)
owns what `workgraph` adds above the cell, and
[scheduler-library.md](../design/scheduler-library.md) owns koan's side of the
stack.
