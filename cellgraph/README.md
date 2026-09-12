# cellgraph

*(working name — the crate fixes a concept, not a final identifier)*

A substrate of computation **cells**. A cell is one unit of suspended
computation: an identity, a region of safely allocated memory, an optional
continuation to run, and the holds that keep other cells alive on its behalf.

The crate names no type from its embedders. The dependency direction is
`koan` → `workgraph` → `cellgraph`, each arrow compile-enforced, so a cell
carries no embedder vocabulary: a continuation family enters as a type
parameter and is stored erased. [workgraph](../workgraph/README.md) is the
first embedder and [koan](../README.md) sits above that.

This README is the substrate's design. Two parts of it are large enough to
carry their own:

- **[src/graph/README.md](src/graph/README.md)** — liveness. The bit matrices,
  the sealed tier, reach, the seal transition, the invariants and the staleness
  argument, the merges, and what retention costs.
- **[src/tree/README.md](src/tree/README.md)** — the tree pool: the third
  region habitat, for a call subtree whose liveness is a stack discipline
  rather than a matrix reading.

## What the substrate is, and what it refuses to be

The substrate does three things: it owns regions, it records which region's
storage is reached from where, and it hands a step a scoped, borrow-checked
door onto both. It does not run anything.

It **never runs a cell**, never chooses an order, and never inspects a
continuation. It names no thread model and no async runtime: it is a
single-mutator structure, and any concurrency is an arrangement above it.
Everything that makes a *scheduler* — dependency edges, wakeups, terminals,
delivery — is the embedder's.

Four absences are design statements rather than gaps:

- **No acyclicity of references.** Cells may reference each other freely. The
  *hold* graph must be acyclic for a cell ever to reclaim, but nothing enforces
  that: a ring keeps both cells alive forever, which leaks rather than dangles.
  A locality merge dissolves a ring it happens to meet, because a hold on one's
  own target has no representation — but that is a coincidence of the merges'
  triggers, not collection. Preventing rings is the embedder's crossing
  discipline; `is_empty` is the alarm for the ones that survive.
- **No terminality, and therefore no error type.** Death is declared, not
  inferred, and carries no result. "Finished forever, with this value" is a
  scheduler's terminal protocol.
- **No admission policy.** `create` refuses at the cap; what to do then — admit
  lazily, drain first, fail the program — is the layer above's.
- **No frame payload.** There is no frame type. Per-cell embedder structure is
  composed *from* cells: a body cell and a storage-only cart cell, held together
  by ordinary holds, is how an embedder gives one unit of work two regions with
  different lifetimes.

## The cell

- **Identity** is a handle: a place plus the generation its occupant held when
  the handle was minted ([src/handle.rs](src/handle.rs)). `Copy`, never an
  owning pointer. An operation on a handle whose occupant has died is an error,
  never a silent no-op — a stale handle means a caller kept a name past a death
  it declared. `SlabHandle` and `TreeHandle` name the two habitats and
  `CellHandle` is the two under one name, which is what a door taking a
  destination or a parent asks for.
- **Habitat**, one of two live kinds. A cell either takes a slab slot, where the
  liveness matrices decide its death, or it is a
  [tree cell](src/tree/README.md), living under a slab **root** through a chain
  of tree parents, in an uncapped pool that no mask and no relation ever names.
  Which kind a creation takes is an admission decision, and the embedder's: the
  substrate ships both and no rule.
- **Region** — one bump per cell ([src/region.rs](src/region.rs)), in
  pointer-stable chunks, and the only place a value with reach may rest.
  Storage can leave its slot without a byte moving, which is what sealing and
  every merge rely on. Nothing in a region is ever dropped, because a bump
  releases its chunks whole: every family a region hosts is `DropFree`. A
  region is a *bundle* — the bump it writes into, plus the bumps of everything
  absorbed into it. Inside a step the executing cell's own region is reachable
  at its own brand, `'cell`, distinct from the step's.
- **Continuation**, optional. An erased, reattachable one-shot the substrate
  stores and hands back under `enter`, re-anchored at the step lifetime, and
  **never calls**. It is stored at `'cell` and records no reach of its own: a
  cell holds what its continuation reads, and every reference the
  continuation can capture is one the cell's holds already cover — its own
  region, or storage a pinned crossing minted in when it arrived — so the
  store prices nothing and touches no table. A cell with no continuation is
  **storage-only**: the answer to "a
  region that outlives its step but is never executed in" — a cart a loop
  accumulates into, a mailbox a scheduler parks values in.
- **Holds**, in two relations — *birth* (the parent chain, derived at creation)
  and *pin* (value reach, accruing as values are minted in). Both are monotone
  for the cell's life and both release wholesale rather than per reason. The
  structures and the discipline are [src/graph/README.md](src/graph/README.md).

## The contract: two embedder types

Everything an embedder knows that the substrate does not rides in one of two
type parameters.

**Continuation** — the work. A one-lifetime reattachable family
([src/reattach.rs](src/reattach.rs)): an erase-to-`'static` storage form and a
single lifetime-retype back. A cell's name-resolution state, its semantic
frame, any output obligation — all of it rides inside the continuation's
captures, or as a value at rest in the cell's region.

**Value** — what passes between cells. Also a one-lifetime reattachable family,
carried witnessed: born in a region, duplicated per reader, read only under a
hold. It is held in exactly three states, and **the type of each is what says
which** — they are named in order of liveness:

- **`Dormant`** ([src/dormant.rs](src/dormant.rs)), at rest — lifetime-free,
  opaque, and the only state an embedder may keep across an `enter` scope. It
  carries no reach: its mask lives in its home cell's reach table and it names
  that entry by a private key. It carries no live *value* either — the bytes
  rest parked, reconstituted only once a redeem has established a claim on the
  storage they name, because a reference into freed chunks is invalid the moment
  it is moved, read through or not.
- **`Ready`** ([src/carrier.rs](src/carrier.rs)), in step — the value bundled
  with the reach the substrate composed for it, branded to the step that built
  or redeemed it. It dies with the step.
- **`Active`** — the value alone, at a reading borrow strictly inside the step.

`'home` is the brand that carries the whole safety argument: the cell whose
region stores this value is live, and its storage fixed-address, for all of
`'home`. Every read rests on that, which is why no read takes a proof of
liveness, and the brand a step's doors hand out is the step's own — so "a
carrier is reachable only inside an `enter` scope" is a lifetime rather than a
rule.

**Two brands per step.** `'b` is the step: a carrier branded to it was built
or redeemed by this step's doors and dies with the step. `'cell` is the
executing cell's own region: invariant, quantified per `enter`, with no
outlives relation to `'b`. A value built there is held as a plain `&'cell`
reference and needs no carrier, because its reach is the cell itself and the
cell's birth row already keeps it; the three carrier states are for a value
homed in another cell or crossing a step. The continuation's captures are
`'cell` references, re-anchored at each step's brand, which is how per-cell
embedder structure rides the cell without a frame type. A foreign carrier is
read at a borrow strictly inside the step and can never coerce to `'cell`, so
the only reference that lands in a cell's region without passing the verdict
is one into that same region.

**A value and its reach are never separable, and never forgeable.** Every
carrier constructor is crate-private and the mask type is crate-private too, so
there is nothing an embedder can assemble that would hand a value a reach of its
own choosing. That single forgery is what the three states exist to prevent.

## Verbs

- **`create(parent?)`** hands back a handle, or refuses when the slab is at its
  cap.
- **`enter(handle, step)`** sets the cell's executing bit for the scope of
  `step` and supplies a step context. A cell cannot be entered while it is
  already executing. Within the scope a step can take the cell's continuation
  re-anchored at `'cell`; take a `Copy` writer onto its own region at `'cell`;
  allocate into any other live cell by handle (destination-homed placement),
  or into itself at `'cell`; lift an own-region value to a carrier whose reach
  is the cell itself; mint a bare hold on another cell; read a carrier it
  built; store a successor continuation, over captures or over nothing; `keep`
  a carrier it holds, which hands back the at-rest form; and `redeem` one a
  previous step put to rest.

  The continuation read *is* the sealed tier's accessor — a capture whose region
  sealed since it was stored comes back reading storage that sealed cell still
  retains — so there is no second door out of sealed storage.

- **`redeem`** is the one door out of the at-rest state, and it **refuses rather
  than panics**. The executing cell must be entitled to the storage the value
  names: it is the home itself, its pin row or its birth row names the home, or
  the home has sealed into a sealed cell this cell holds. For a value homed in a
  tree cell the test is root identity. Anything else is `Unheld`; a home whose
  storage is gone entirely is `Gone`. Nothing could have read such a value, so
  nothing is lost by refusing it.
- **`create_tree` / `release_tree`** are birth and death over the tree pool;
  `enter` is one door over both kinds. See
  [src/tree/README.md](src/tree/README.md).
- **`release(handle, absorption)`** declares death: the embedder promises never
  to enter the cell again. The slot leaves the slab once no descendant's birth
  row names it — reclaimed if nothing reaches its storage, folded into the one
  thing that reaches it if there is exactly one, sealed otherwise. `absorption`
  is the embedder's say over that fold, recorded on the slot and read when the
  slot actually leaves.
- **`is_empty()`** asks whether the graph holds nothing at all — every slot
  free, no sealed cell left, no tree cell or tombstone left in the pool. After a
  program's last release it is the end-of-program alarm, and the only one the
  substrate ships: a non-empty graph means a release was forgotten or a ring no
  merge dissolved survives. Naming the nodes on such a ring is a walk of the
  hold graph the crate's own tests carry, not a door.

### One scratch region, reset at entry

Every verb runs over the graph's **scratch region**
([src/scratch.rs](src/scratch.rs)): one bump per graph, reset at the entry of
`create`, `enter` and `release` and never inside one. Every transient a verb
builds lives there — the worklists a disposal cascade nests, the runs a
placement builds per operand, the views a build closure receives — so a
transient lives exactly as long as the verb that built it, and a verb on a graph
whose region is already warm asks the allocator for nothing.

The reset sits at a verb's *entry* rather than its exit because a `release`
cascades outside any step: what the region has to be is empty when a verb
starts, not when the one before it finished. A reset runs no destructor, so
nothing with drop glue goes in it — the same rule the cell regions keep, and
asserted the same way at compile time.

## Passing values between cells

There is no delivery protocol. A value always rests in a live cell's region, is
transient inside an executing cell's step, or is sealed with its region. Nothing
else holds a value: **there is no free-standing envelope with pins of its own**,
and that absence is what the staleness argument in
[src/graph/README.md](src/graph/README.md) turns on.

Crossing a step boundary therefore takes one of two shapes, both built from the
verbs above, and both completing through `keep` on the producing side and
`redeem` on the reading one:

- **Push.** While the producer executes, the value is minted into the consumer's
  region — or built there outright by destination-homed placement — and the
  consumer's row takes its mask. The producer keeps the carrier and hands the
  at-rest form to the embedder, which delivers it; the consumer redeems it in a
  later step of its own. The producer can then die at column zero.
- **Pull.** The consumer mints a bare hold on the producer cell. The producer
  dies with a nonzero column and seals, and the consumer redeems later: the home
  resolves to the sealed cell, the consumer's hold on it is the entitlement, and
  the value comes back reaching that sealed cell's id alone.

Which shape an edge takes is the embedder's choice, per edge, and delivering the
at-rest carrier is the embedder's job too — the substrate ships no queue and no
mailbox, only the two doors.

### The crossing verdict

A placement over operands is where the copy-versus-pin choice is made, and the
substrate does not make it. The graph is constructed with one embedder
closure — the **crossing verdict** — and consults it once per operand of every
placement, the destination-homed placement and the capturing successor store
alike, including operands whose pin price is zero. There is no verdict-free
constructor: a graph that can place a value can price the placement.

The closure is shown both halves of the price and the occupancy the choice plays
out against: the bytes a pin would *newly* keep alive (marginal against what the
destination already holds and against what earlier operands of this same
placement have already pinned, so the prices sum to what the placement retains);
the copy cost, which only the embedder can know and passes beside the operand;
the slab's occupancy against its cap, the sealed tier's count and retained bytes,
and the destination region's own size. **The substrate ships numbers and no
threshold** — whether the ramp is linear on occupancy or a step at a watermark
is the embedder's.

What comes back decides the shape the build closure receives, and the type
system enforces it:

- a **pinned** operand arrives at the destination's own region brand, so the
  build may embed the borrow itself — and the mint has already folded the
  operand's reach into the destination's holds;
- a **copied** operand arrives **severed**, at a brand with no outlives relation
  to the destination's region, and its reach is minted nowhere. Embedding it is
  a compile error, so the only copy that typechecks is a deep one through the
  destination's writer.

Both shapes reach the build closure out of the scratch region and neither
outlives the call: their brands are quantified over the call, so a caller has
nowhere to put a view it kept.

The verdict is skipped in exactly one case, and only because there is no choice
to put: an operand homed in a tree cell crossing to a destination neither on
that cell's chain nor under it is a **forced copy**
([src/tree/README.md](src/tree/README.md)).

A `'cell` reference is not an operand: embedding it prices nothing because it
crosses nothing.

There are no price *verbs*. The substrate computes what retention costs, but
every one of those queries is crate-private, along with the vocabulary they
speak — the mask, the sealed id, the closure and occupancy answers name nothing
an embedder can hold. A price returns to the embedder at exactly one place, for
exactly one decision.

## Source layout

- [src/lib.rs](src/lib.rs) — the module wiring and the public surface.
- [src/handle.rs](src/handle.rs) — cell identity over both habitats, and the
  stale refusals.
- [src/graph.rs](src/graph.rs) — the slab, the verbs, the step context's doors,
  the seal transition, the three locality merges, the disposal cascade, and the
  relocation map that forwards a dormant carrier through a merge. The embedder's
  crossing verdict is taken here at construction.
- [src/tree.rs](src/tree.rs) — the tree pool: chain links and depth, the
  undisposed-child count, the pledge, and the tombstone chain.
- [src/matrix.rs](src/matrix.rs) — `Bits`, the crate's one row of bits, and the
  two relations as inline arrays of those rows.
- [src/reach.rs](src/reach.rs) — reach as a hybrid mask: an inline `Bits` row
  over slab slots plus a sparse sealed-id set, itself inline up to two ids.
- [src/sealed.rs](src/sealed.rs) — the sealed tier: ids as a serial beside a
  slab index, sparse sets, frozen aggregates, holder counts, and the dense slab
  with its free list.
- [src/region.rs](src/region.rs) — the per-cell bundle of bumps, the splice a
  merge performs, `Writer` — the crate's one write surface: `fill`, a run laid
  down by index under a compile-time no-destructor check, and `text`; every
  simpler shape is the embedder's — and the sealed cell's frozen-closure memo.
- [src/scratch.rs](src/scratch.rs) — the graph's one scratch region and the
  doors every verb's transients go through.
- [src/carrier.rs](src/carrier.rs) — `Ready` and `Active`, the two carrier
  states that carry a lifetime.
- [src/dormant.rs](src/dormant.rs) — `Dormant`, the private key naming its
  reach, and the per-cell reach table that reach lives in.
- [src/reattach.rs](src/reattach.rs) — the reattachable contract and the single
  lifetime-retype the crate is built on.
- [tests/surface.rs](tests/surface.rs) — the public surface, named and exercised
  from outside the crate. Everything an embedder may reach is used here and
  nothing else is reachable to use, so an item that widens shows up as an unused
  import and one that goes missing as a compile error.
  [tools/verify.sh](../tools/verify.sh) runs it under `--release` as well, since
  a surface that changed shape with the build profile would compile for an
  embedder in one profile and not the other.

## Failure direction, and what the posture follows from

A reference count fails safe: a forgotten release leaks. This model fails
dangerous: a forgotten bit reclaims a live cell. That trade is accepted
deliberately, and it dictates the engineering posture. The matrices, the sealed
tier and every hold transition are encapsulated behind an interface designed so
that safe usage *cannot skip a declaration* — a value cannot be stored without
its mask passing through the mint OR, and sealed contents cannot be read except
through the accessor, which hands back no mask to re-pair. The core is tested
exhaustively (property tests over hold/seal/retire interleavings, plus the Miri
slate) rather than audited by convention, and the surface is narrow in the
literal sense: what an embedder can name is fixed by an integration test that
names all of it, run under both build profiles.

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
  sub-tier wound down, a cell kept into at many distinct reaches, and a chain of
  tree cells each pinning its result into its parent — and
  [perf/meter.rs](perf/meter.rs) the meter, which subtracts a nested door's spend
  from its parent's frame. It counts through
  [audit/counting_alloc.rs](../audit/counting_alloc.rs), the same delegating
  allocator koan's own readings go through.
- [tools/cellgraph_perf.py](../tools/cellgraph_perf.py) — the one command.
  `python3 tools/cellgraph_perf.py` sweeps the set and prints a delta against the
  newest recorded commit, which it rebuilds and runs beside HEAD so the time
  column is a comparison taken in one sitting rather than a figure written down
  in another. `--record` appends HEAD's readings, `--gate` exits non-zero if
  allocations or bytes rose, and `--gate-time` exits non-zero if any row's
  fastest trial sits more than 10 % above the rebuilt baseline's. Every row is
  weighed: the harness runs each shape in blocks sized so the smallest row's
  block clears 20 µs and reports the fastest block per run, and the tool execs
  every trial from a fresh copy of its binary so neither side reads from one
  fixed placement of its pages. The tolerance comes from `--calibrate`, which
  sweeps HEAD against a rebuild of HEAD — identical source, so every row's
  movement is this machine's own spread.
- [observe/perf.csv](observe/perf.csv) — the record: a tidy dataframe, one row
  per `(date, sha, dirty, benchmark, n, cap, verb)` carrying `calls`,
  `allocations`, `bytes` and `nanos`, capped to the three most recently recorded
  commits. Allocations and bytes are deterministic and gate a change on their
  own; a recorded `nanos` is there for the trend and is never asserted, since it
  was read in another session on a machine doing other things.

## Open work

- [roadmap/](roadmap/README.md) — the crate's own tree. The substrate's
  build-out is complete; what is open is recorded there as unplanned gaps.
- [Cell brand and writer doors](roadmap/cell-brand-and-writer.md) — the
  `'cell` brand, `writer`, `fill` and the carrier bridge described above.
- [Rebuilding workgraph over cellgraph](../workgraph/old_roadmap/adopt-cellgraph.md)
  — the first embedder's adoption.

Docs that state the *boundary* between the substrate and an embedder stay with
the embedder: what a scheduler adds above the cell is the embedder's design to
write, not this crate's.
