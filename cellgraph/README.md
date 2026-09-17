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

- **[src/graph/README.md](src/graph/README.md)** — liveness. The bit matrix,
  the sealed tier, reach, the seal transition, the invariants and the staleness
  argument, the merges, and what retention costs.
- **[src/tree/README.md](src/tree/README.md)** — the tree pool: the third
  region habitat, for a call subtree whose liveness is a stack discipline
  rather than a matrix reading.

The third kind of cell, the **tenant**, owns no region and so has no design doc
of its own: it is stated here, under [The cell](#the-cell), and its effect on
disposal in [src/graph/README.md](src/graph/README.md).

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
  composed *from* cells, in both directions. One unit of work, two regions: a
  body cell and a storage-only cart cell, held together by ordinary holds, give
  one unit of work two regions with different lifetimes. Many units of work,
  one region, at the embedder's election: a **tenant** cell owns no region and
  writes its host's, so work whose results share structure runs in one region
  and embeds across it at no price.

## The cell

- **Identity** is a handle: a place plus the generation its occupant held when
  the handle was minted ([src/handle.rs](src/handle.rs)). `Copy`, never an
  owning pointer. An operation on a handle whose occupant has died is an error,
  never a silent no-op — a stale handle means a caller kept a name past a death
  it declared. `SlabHandle`, `TreeHandle` and `TenantHandle` name the three
  kinds of cell and `CellHandle` is the three under one name, which is what a
  door taking a destination, a parent or a host asks for.
- **Habitat**, one of two live kinds. A cell that owns a region either takes a
  slab slot, where the pin matrix decides its death, or it is a
  [tree cell](src/tree/README.md), living under a slab **root** through a chain
  of tree parents, in an uncapped pool that no mask and no relation ever names.
  Which kind a creation takes is an admission decision, and the embedder's: the
  substrate ships both and no rule.
- **Tenant**, the kind with no habitat ([src/tenant.rs](src/tenant.rs)). A
  tenant has what a step needs — a continuation, an executing flag, a
  generation — and no region: it names a **host**, a slab or tree cell, and a
  step in it is handed the host's writer. It places, mints and lifts as the
  host, its carriers carry the host's reach, and it redeems exactly what the
  host may, so a value it writes embeds a host-homed `'here` borrow with no
  operand, no pin and no price. No value is ever homed in a tenant. A live
  tenant named where a place with storage is asked for — a host, a tree parent,
  a placement destination — means its host, whether or not the host's own death
  has been declared, so a chain of tenants each created by naming the one
  before it shares the first host's region for as long as any of them lives.
  Tenants sit in an uncapped pool of their own, in no relation. When to make a
  cell a tenant is the embedder's election: the substrate ships the kind and no
  rule.
- **Region** — one bump per region-owning cell ([src/region.rs](src/region.rs)), in
  pointer-stable chunks, and the only place a value with reach may rest.
  Storage can leave its slot without a byte moving, which is what sealing and
  every merge rely on. Nothing in a region is ever dropped, because a bump
  releases its chunks whole: every family a region hosts is `DropFree`. A
  region is a *bundle* — the bump it writes into, plus the bumps of everything
  absorbed into it. Inside a step the executing cell's own region is reachable
  at its own brand, `'here`, distinct from the step's.
- **Scratch habitat** — a second bump beside each region, written at a brand of
  its own, `'scratch`, for what a step can prove it throws away: the multi-step
  transients a long-lived cell would otherwise strand in the region it keeps
  until it dies. It is no part of the region — it never seals, splices or
  absorbs, no price counts its bytes, and it stays at its table index when the
  region leaves — and it is handed back whole at the first `enter` that finds
  nothing naming it. A departing cell's scratch is dropped at its disposal. A
  tenant's scratch is its host's.
- **Continuation**, optional. An erased, reattachable one-shot the substrate
  stores and hands back under `enter`, re-anchored at the step lifetime, and
  **never calls**. It rests in two halves, each in its own slot: the **storage
  half**, captured at `'here`, and the **scratch half**, captured at `'scratch`,
  which is what carries a scratch structure across a park and is empty at
  birth. The storage half is captured at `'here` and records no reach of its own: a
  cell holds what its continuation reads, and every reference the
  continuation can capture is one the cell's holds already cover — its own
  region, or storage a pinned crossing minted in when it arrived — so the
  store prices nothing and touches no table. A cell with no continuation is
  **storage-only**: the answer to "a
  region that outlives its step but is never executed in" — a cart a loop
  accumulates into, a mailbox a scheduler parks values in.
- **Holds**, in one relation — *pin*: value reach, accruing as values are minted
  in. It is monotone for the cell's life and releases wholesale rather than per
  reason. A slab cell stands under nothing; the parent chain is the tree pool's,
  and it is no relation at all. The structure and the discipline are
  [src/graph/README.md](src/graph/README.md).

## The contract: two embedder types

Everything an embedder knows that the substrate does not rides in one of two
type parameters.

**Continuation** — the work. A reattachable family
([src/reattach.rs](src/reattach.rs)) over one region lifetime, `'cell`, beside
the graph lifetime `'graph`: an erased storage form at `'graph` and a single
lifetime-retype that moves `'cell` alone. A cell's name-resolution state, its
semantic frame, any output obligation — all of it rides inside the
continuation's captures, or as a value at rest in the cell's region. The
continuation's scratch half is a family of its own, `S`, which defaults to the
continuation family: a separate family keeps the wrong half unrepresentable in
each slot and sizes each slot to its own half, and an embedder that parks
nothing in scratch never names it. The reattachable contract is the same for
both — one lifetime per family.

**Value** — what passes between cells. Also a reattachable family,
carried witnessed: born in a region, duplicated per reader, read only under a
hold. It is held in exactly three states, and **the type of each is what says
which** — they are named in order of liveness:

- **`Dormant`** ([src/dormant.rs](src/dormant.rs)), at rest — free of every
  step brand, opaque, and the only state an embedder may keep across an `enter`
  scope. It carries no reach: its mask lives in its home cell's reach table and
  it names that entry by a private key. It carries no live *value* either — the
  bytes rest parked, reconstituted only once a redeem has established a claim on
  the storage they name, because a reference into freed chunks is invalid the
  moment it is moved, read through or not.
- **`Ready`** ([src/carrier.rs](src/carrier.rs)), in step — the value bundled
  with the reach the substrate composed for it, branded to the step that built
  or redeemed it. It dies with the step.
- **`Active`** — the value alone, with no reach: at a reading borrow strictly
  inside the step when a read hands it back, and at the destination's region
  brand when a build closure hands back the value it built.

`'home` is the brand that carries the whole safety argument: the cell whose
region stores this value is live, and its storage fixed-address, for all of
`'home`. Every read rests on that, which is why no read takes a proof of
liveness, and the brand a step's doors hand out is the step's own — so "a
carrier is reachable only inside an `enter` scope" is a lifetime rather than a
rule.

**Three brands per step, under the graph's lifetime.** `'step` is the step: a
carrier branded to it was built or redeemed by this step's doors and dies with
the step. `'here` is the executing cell's: invariant, quantified per `enter`,
with no outlives relation to `'step`, and naming storage the cell's hold set
covers for the cell's whole life — its own region, or a region a pinned crossing
into this cell minted into its holds. It is a real borrow, not a brand alone:
the shared borrow of the graph's region table that `enter` holds for the whole
step beside its exclusive borrow of everything else, so the step's own writer is
a plain `&'here` and the verbs that move or drop a region cannot run under it. A
value built there is held as a plain `&'here` reference and needs no carrier,
because its reach is the cell itself, which is alive for as long as the borrow;
the three carrier states are for a value homed in another cell or crossing a
step. The continuation's captures are `'here` references, re-anchored at each
step's brand, which is how per-cell embedder structure rides the cell without a
frame type. For a step in a tenant `'here` is the host's: the host's region and
the host's hold set, which the host keeps until its last tenant has left.

`'scratch` is the scratch habitat's: invariant and quantified per `enter` like
`'here`, with `'here: 'scratch` the whole of its relation to it. So a scratch
value may reference `'here` storage, and a scratch borrow cannot go anywhere
that asks for `'here` — which every door into storage does. Embedding one is a
compile error by every route: the successor store, the region writer's output
used at `'here`, `lift`, a placement's build into another cell or into the
executing cell itself, the return of `enter`, and a write through a
`'here`-homed slot. Each is pinned by a `compile_fail` doctest on
`scratch_writer` against one compiling control. The trip is one way and the
habitat's only door across a park is the scratch half of the continuation. Two
limits follow from one lifetime per family. An invariant `'here` structure
(`&'here Table<'here>`) cannot ride the scratch half, since it would have to
shorten to `'scratch`; it stays in the storage half and the step rejoins the
two each wake. And the region's own writer is covariant, so its output used at
`'scratch` is sound and useless: the bytes land in storage, stranded until the
cell dies.

Both halves are re-anchored in `enter`, the one function that mints both brands
and both writers, so the pairing of slot with brand is made and audited in one
place and the continuation doors are field moves.

`'graph` outlives both. It is the lifetime of storage the embedder owns outside
the graph — program text, say — which the borrow checker keeps alive for as
long as the graph is used; the graph is invariant in it, so it never shortens to
a step's brand. Anything that outlives the graph may be borrowed through it: a
continuation captures a `&'graph` borrow, a build embeds one, and a value's form
may nest one under a region borrow (`&'cell Entry<'graph, 'cell>`). It is not a
brand — nothing is confined to it — and it carries **no reach and no price**:
the substrate neither keeps nor reclaims that storage, so a `'graph` borrow is
minted into no hold set, weighed by no verdict, and crosses a copy as it is,
since the retype never moves it. An embedder with no such storage writes
`CellGraph<'static, C>`.

A foreign carrier is read at a borrow strictly inside the step and can never
coerce to `'here`, so the only references that land in a cell's region without
passing the verdict are ones into that same region or through `'graph`.

**A value and its reach are never separable, and never forgeable.** The
constructors of `Ready` and `Dormant`, the two states that carry reach, are
crate-private and the mask type is crate-private too, so there is nothing an
embedder can assemble that would hand a value a reach of its own choosing. That
single forgery is what the three states exist to prevent. `Active::new` is
public, because an `Active` holds no reach and no door takes one as evidence of
anything: a placement's build closure ends in one. It hands back an `Active`
rather than the bare form because the build is quantified over the destination's
`'cell`, and a closure so quantified cannot prove `'graph: 'cell` of the type it
returns. So `Active` holds the value erased beside an invariant `'cell` marker:
its type needs no bound, and the bound sits on `Active::new`, which erases, and
on the read out, which re-anchors at the same `'cell`.

## Verbs

- **`new(cap, verdict)` / `with_config(config, verdict)`** build the graph.
  `Config` carries the slab's cap and the two constants of the spare list's
  bound — a proportion and the window of a moving average, below — which are
  runtime values beside the type-level width `W`.
- **`create(continuation?)`** hands back a handle, or refuses when the slab is
  at its cap. The new cell stands on its own — a slab cell is under nothing. A
  continuation handed in at birth is at `'graph`: it borrows no region, so it
  reaches nothing.
- **`enter(handle, step)`** sets the cell's executing bit for the scope of
  `step` and supplies a step context. A cell cannot be entered while it is
  already executing. Within the scope a step can take the cell's continuation
  re-anchored at `'here` (`continuation`), and its scratch half re-anchored at
  `'scratch` (`scratch_continuation`); take a `Copy` writer onto its own region
  at `'here`, and one onto its scratch habitat at `'scratch`
  (`scratch_writer`);
  allocate into any other live cell by handle (destination-homed placement),
  or into itself at `'here`; lift an own-region value to a carrier whose reach
  is the cell itself; mint a bare hold on another cell; read a carrier it
  built; store a successor continuation, over captures or over nothing, and a
  scratch successor beside it (`store_scratch_successor`); `keep`
  a carrier it holds, which hands back the at-rest form; and `redeem` one a
  previous step put to rest.

  The continuation read *is* the sealed tier's accessor — a capture whose region
  sealed since it was stored comes back reading storage that sealed cell still
  retains — so there is no second door out of sealed storage.

- **`redeem`** is the one door out of the at-rest state, and it **refuses rather
  than panics**. The executing cell must be entitled to the storage the value
  names: it is the home itself, its pin row names the home, or the home has
  sealed into a sealed cell this cell holds. For a value homed in a
  tree cell the test is root identity. Anything else is `Unheld`; a home whose
  storage is gone entirely is `Gone`. Nothing could have read such a value, so
  nothing is lost by refusing it.
- **`create_tree` / `release_tree`** are birth and death over the tree pool;
  `enter` is one door over all three kinds. See
  [src/tree/README.md](src/tree/README.md).
- **`create_tenant(host, continuation?)` / `release_tenant`** are birth and
  death over the tenant pool. Birth draws no bump and takes no slab slot, so it
  has no full refusal; it refuses only a stale name. Death is **a count
  decrement on the host** — no reclaim, no splice, no pledge, no tombstone —
  because a tenant owns nothing to settle: what it wrote is the host's and
  stays the host's. A tenant has no dead-but-undisposed state, since nothing
  can be under one. If the host's own death was already declared and this
  tenant was the last thing it waited on, the host disposes within the call.
- **`release(handle, absorption)`** declares death: the embedder promises never
  to enter the cell again. The slot leaves the slab within that same call —
  reclaimed if nothing reaches its storage, folded into the one thing that
  reaches it if there is exactly one, sealed otherwise — unless a tree cell
  under it has not disposed or a tenant is still writing its region, the two
  things that make a death outlive its own `release`. `absorption` is the embedder's say over that fold, recorded
  on the slot and read when the slot actually leaves.
- **`is_empty()`** asks whether the graph holds nothing at all — every slot
  free, no sealed cell left, no tree cell or tombstone left in the pool, and no
  tenant. After a
  program's last release it is the end-of-program alarm, and the only one the
  substrate ships: a non-empty graph means a release was forgotten or a ring no
  merge dissolved survives. Naming the nodes on such a ring is a walk of the
  hold graph the crate's own tests carry, not a door.

### Recycled regions

A reclaimed region's chunks do not go back to the allocator: each bump of the
bundle is reset onto a graph-level **spare list**, and `create` and
`create_tree` draw from it, so a release-then-create loop settles at zero
allocator calls per hop. The list is last in, first out, which is what makes a
tail hop write into the chunk its predecessor just gave up. Every reclaim path
retires through it — a slab reclaim, an unpledged tree disposal, a sealed
cell's retirement, and the bump a splice leaves out of a bundle. Recycling is
allocator-private: no verb names it and none of the birth and death verbs
carries it in its signature.

The list is bounded against recent demand, so a program's peak does not stay
resident for the rest of its run. It holds at most `spare_proportion` times a
moving average of the live region-owning cell count, rounded up, and a bump
retired past that goes back to the allocator. The average is fixed point,
sampled at every birth and every disposal of a region-owning cell and nowhere
else — no clock and no float — and closes `1 / 2^spare_window_shift` of its gap
to the live count per sample. Tenants, sealed storage and absorbed bumps are
not counted: spares serve births, and those are retention rather than demand.
The bound is enforced where a bump is pushed, so a list an earlier peak left
long drains a bump per birth rather than in a trim pass. The defaults are a
proportion of 2 — a sawtooth between nothing and a peak averages half the peak
and wants the whole peak spare at its trough — and a shift of 6; a proportion
of 0 recycles nothing. The safety argument and the `cfg(miri)` switch are
[src/graph/README.md § References at `'here`](src/graph/README.md#references-at-here).

### One scratch region, reset at entry

The scratch region is the *graph's* and strictly inside a verb; it is not the
per-cell [scratch habitat](#the-cell), which is the embedder's to write and
lives across steps.

Every verb runs over the graph's **scratch region**
([src/scratch.rs](src/scratch.rs)): one bump per graph, reset at the entry of
every birth verb, `enter` and every death verb, and never inside one. Every transient a verb
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

- a **pinned** operand arrives at the destination's own region brand, `'cell`,
  so the build may embed the borrow itself — and the mint has already folded the
  operand's reach into the destination's holds;
- a **copied** operand arrives **severed**, at `'severed`, a brand with no
  outlives relation to `'cell`, and its reach is minted nowhere. Embedding its
  region part is a compile error, so the only copy that typechecks is a deep one
  through the destination's writer. Severing is by lifetime and `'graph` is not
  a region's, so a `'graph` borrow inside a copied view embeds as it is.

Both shapes reach the build closure out of the scratch region and neither
outlives the call: their brands are quantified over the call, so a caller has
nowhere to put a view it kept.

The verdict is skipped in exactly one case, and only because there is no choice
to put: an operand homed in a tree cell crossing to a destination neither on
that cell's chain nor under it is a **forced copy**
([src/tree/README.md](src/tree/README.md)).

A `'here` reference is not an operand: embedding it prices nothing because it
crosses nothing. Nor is a `'graph` borrow, for the same reason.

There are no price *verbs*. The substrate computes what retention costs, but
every one of those queries is crate-private, along with the vocabulary they
speak — the mask, the sealed id, the closure and occupancy answers name nothing
an embedder can hold. A price returns to the embedder at exactly one place, for
exactly one decision.

## Source layout

- [src/lib.rs](src/lib.rs) — the module wiring and the public surface.
- [src/handle.rs](src/handle.rs) — cell identity over all three kinds, the
  crate-private name for the two kinds a value can be homed in, and the stale
  refusals.
- [src/graph.rs](src/graph.rs) — the slab, the graph's `Config`, the verbs over
  all three kinds, the write-home resolution every door naming a place with
  storage goes through, the step context's doors,
  the seal transition, the three locality merges, the disposal cascade, and the
  relocation map that forwards a dormant carrier through a merge. The embedder's
  crossing verdict is taken here at construction. The graph is two halves a
  step borrows apart: the cells — identity, relations, holds, the sealed tier —
  exclusively, and the region table shared.
- [src/tree.rs](src/tree.rs) — the tree pool: chain links and depth, the
  undisposed-child count, the pledge, and the tombstone chain.
- [src/tenant.rs](src/tenant.rs) — the tenant pool, and the two counts a host
  carries for its tenants.
- [src/matrix.rs](src/matrix.rs) — `Bits`, the crate's one row of bits, and the
  pin relation as an inline array of those rows.
- [src/reach.rs](src/reach.rs) — reach as a hybrid mask: an inline `Bits` row
  over slab slots plus a sparse sealed-id set, itself inline up to two ids.
- [src/sealed.rs](src/sealed.rs) — the sealed tier: ids as a serial beside a
  slab index, sparse sets, frozen aggregates, holder counts, and the dense slab
  with its free list.
- [src/region.rs](src/region.rs) — the per-cell bundle of bumps, the splice a
  merge performs, `Regions` — the table of every live cell's region, which a
  step holds shared for its whole length so that nothing can move or drop a bump
  under a writer into it, with the scratch bumps beside the regions and the
  bounded spare list — `Writer` — the crate's one write surface, a verb per
  shape a region cannot be given: `fill`, `thin_run` and `text` where the width
  is settled before the first element — `thin_run` laying its run behind a
  length header so its `ThinRun` handle is one pointer wide — `run` and `prose` where only the producer settles it,
  each handing back the region borrow once the producer is done; every simpler
  shape is the embedder's — and the sealed cell's frozen-closure memo.
- [src/scratch.rs](src/scratch.rs) — the graph's one scratch region and the
  doors every verb's transients go through.
- [src/carrier.rs](src/carrier.rs) — `Ready` and `Active`, the two carrier
  states that carry a lifetime beside `'graph`.
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
deliberately, and it dictates the engineering posture. The matrix, the sealed
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
machinery and not the `alloc_into` within it.

- [perf/](perf/) — the harness, a `[[bin]]` behind the `perf` cargo feature so
  the library build, its tests, and the Miri slate never compile it.
  [perf/shapes.rs](perf/shapes.rs) holds the shapes — a keep-and-redeem loop, a
  push chain, a pull chain, a fan-out placement, a shared
  sub-tier wound down, a cell kept into at many distinct reaches, and a chain of
  tree cells each pinning its result into its parent, and `tail_hop`, a
  create-then-release loop whose allocations and resident bytes are the same at
  every length — and
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
  commits. A shape that reports what it leaves resident does so as a row under
  the pseudo-verb `resident`, whose `bytes` is the thread's live bytes at the
  end of the workload over its start. Allocations and bytes are deterministic and gate a change on their
  own; a recorded `nanos` is there for the trend and is never asserted, since it
  was read in another session on a machine doing other things.

## Open work

- [roadmap/](roadmap/README.md) — the crate's own tree. The substrate's
  build-out is complete; what is open is recorded there as unplanned gaps.
- [Rebuilding workgraph over cellgraph](../workgraph/old_roadmap/adopt-cellgraph.md)
  — the first embedder's adoption.

Docs that state the *boundary* between the substrate and an embedder stay with
the embedder: what a scheduler adds above the cell is the embedder's design to
write, not this crate's.
