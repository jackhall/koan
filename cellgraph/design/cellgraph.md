# The cellgraph substrate

*(working name — `cellgraph` fixes a concept, not a final crate identifier)*

A graph of computation **cells**. A cell is one unit of suspended
computation: an identity, a region of safely allocated memory, an optional
continuation to run, and the holds that keep other cells alive on its behalf.
The graph makes no acyclicity promise about which cells reference which, has
no notion of a cell finishing, and lets a cell live indefinitely — re-entered,
held across arbitrary spans, or never entered at all. Liveness is decided by
the [liveness matrix](liveness-matrix.md). Everything that makes a
*scheduler* — dependency edges, wakeups, terminals, delivery — is layered
above by an embedder; `workgraph` is the first
([dag-scheduler.md](../../workgraph/design/dag-scheduler.md)). The dependency
direction is `koan` → `workgraph` → `cellgraph`, and each arrow is
compile-enforced: the lower crate names no type from the higher one.

## The cell

- **Identity** is a handle: slab slot plus generation, `Copy`. A handle names
  one occupant; an operation on a handle whose occupant has died is an
  error, never a silent no-op, because a stale handle means a caller kept a
  name past a death it declared. A **tree cell** is named the same way over
  its own pool — index plus generation — and `CellRef` is the two of them
  under one name, which is what a door taking a destination or a parent asks
  for.
- **Habitat**, one of two. A cell either takes a slab slot, where the
  liveness matrix decides its death, or it is a
  [tree cell](tree-cells.md): a cell of a call subtree, living under a slab
  **root** through a chain of tree parents, in an uncapped pool that no mask
  and no relation ever names. A tree cell's liveness is structural — a parent
  outlives its children — so it takes no row, no column, no holder count and
  no resident table; a placement into it mints into its root's holds, and its
  region either reclaims at death or splices into an ancestor's bundle. Which
  kind a creation takes is the embedder's admission decision; the substrate
  ships both and no rule.
- **Region.** One bump region per cell, in pointer-stable chunks, is the only
  place a value with reach may rest. Storage can leave the slot without a
  byte moving, which is what the sealed tier and absorption rely on.
- **Continuation**, optional. An erased, reattachable one-shot the substrate
  stores and hands back under `enter`, re-anchored at the step lifetime, and
  never calls. It rests beside the reach of whatever it captured, minted into
  its own cell's holds when it is stored — a cell holds what its continuation
  reads — which makes it a resident like any other: its reach is interned into
  the cell's resident table, rewritten by the seal transition exactly like
  every other entry, and the read that hands it back the sealed tier's
  accessor. Storing a successor repoints the cell at the entry the new reach
  interns to; it overwrites none, so a cell that alternates between a few
  continuation shapes costs one entry per shape. A step may
  store a successor before its scope ends. A cell with no continuation is
  **storage-only**, and is the substrate's answer to
  "a region that outlives its step but is never executed in": a cart a loop
  accumulates into, a mailbox a scheduler parks values in.
- **Holds**, in two relations. *Birth holds* fix the cell's parent chain at
  creation: a cell names at most one parent, and its birth row is the
  parent's row plus the parent's bit, derived by the substrate so transitive
  closure is by construction. *Pin holds* accrue as values with reach are
  minted into the region. Both are monotone for the cell's life, and both
  release wholesale rather than per reason: the birth row at the cell's
  declared death, the pin row when its slot leaves the slab — by clearing if
  the cell reclaims, by freezing into an aggregate if it seals.

## The contract: two embedder types

- **Continuation** — the work. A one-lifetime reattachable family
  (the erase-to-`'static` / re-anchor contract the witnessed core carries,
  [witnessed-memory.md](../../workgraph/design/witnessed-memory.md)).
  Everything an embedder knows about a cell that the substrate does not — its
  name-resolution state, its semantic frame, any output obligation — rides
  inside the continuation's captures, or as a value resident in the cell's
  region.
- **Value** — what passes between cells. A one-lifetime reattachable family
  carried witnessed: born in a region, duplicated per reader, read only under
  a hold. It is held in exactly three states, and the type of each is what
  says which:
  - **At rest** — lifetime-free, opaque, and the only state an embedder may
    keep across an `enter` scope. It carries no reach and no method: its
    borrows rest as bytes in the region it was built into, and a private key
    names the entry of its home cell's resident table where its mask lives.
    That table interns on content, so two values reaching the same thing name
    one entry and a cell kept into every step of a run holds one entry per
    distinct reach rather than one per keep. Born only from an in-step
    carrier, at the `keep` door.
  - **In step** — branded to the step that built or redeemed it, and paired
    with the reach the substrate composed for it. Handed back by the
    placement doors and by `redeem`; it dies with the step.
  - **Active** — the value itself, at a reading borrow strictly inside the
    step.

  The pairing of a value with a reach is only ever one the substrate made:
  the mask is crate-private in every state, so there is nothing an embedder
  can hold that would let it hand a value a reach of its own choosing.

There is no frame type. Per-cell embedder structure is composed from cells: a
body cell and a storage-only cart cell, held together by ordinary holds, is
how an embedder gives one unit of work two regions with different lifetimes.

## Verbs

- **`create(parent?)`** hands back a handle, or refuses when the slab is at
  its cap. What to do on refusal is admission policy, and the embedder's.
- **`enter(handle, step)`** sets the cell's executing bit for the scope of
  `step` and supplies a step context. Within it a step can take the cell's
  continuation, re-anchored at the step lifetime; allocate into its own
  region; allocate into any other live cell by handle (the destination-homed
  placement); mint a bare hold on another cell; read a carrier it built; and
  store a successor continuation, over captures or over nothing. It can also
  put a carrier it holds to rest — `keep`, which hands back the at-rest
  form — and redeem one a previous step put to rest. That
  continuation read *is* the sealed tier's accessor — a capture whose region
  sealed since it was stored comes back reading storage that record still
  retains — so there is no second door out of sealed storage. What the value
  reaches stays in the table: every reach a resident was minted with is the
  substrate's bookkeeping, rewritten in place by the seal transition, and
  never handed back beside the value. A cell cannot be entered while it
  is already executing.

  **`redeem`** is the one door out of the at-rest state, and it refuses
  rather than panics. The executing cell must be entitled to the storage the
  value names: it is the home cell itself, its pin row or its birth row names
  the home — both keep the home in the slab with its storage intact — or the
  home has sealed into a record this cell holds. For a value homed in a tree
  cell the test is root identity, and a step inside a subtree is entitled by
  its root's rows. Anything else is `Unheld`,
  and a home whose storage is gone entirely, reclaimed or retired with its
  record, is `Gone`. Nothing could have read such a value, so nothing is lost
  by refusing it. A value redeemed out of a record comes back reaching that
  record's id alone: a hold on a record keeps its whole aggregate alive
  transitively, so the id covers everything the value reads.

  A placement over operands consults the **crossing verdict** once per
  operand before it builds — the one closure the table was constructed with,
  described under Passing values below.
- **`create_tree(parent)`** / **`release_tree(handle)`** are birth and death
  over the tree pool; `enter` is one door over both kinds. `create_tree` takes
  a cell of either kind as the parent and refuses only a stale one, since the
  pool has no cap; a step entered in a tree cell gets the same context, whose
  placements mint into the root; `release_tree` takes no absorption argument,
  because where a tree cell's bytes go was settled at the placement door that
  pinned a value homed there into an ancestor. A parent released before its
  children waits dead-resident and disposes when the last of them does. See
  [tree-cells.md](tree-cells.md).
- **`release(handle, absorption)`** declares death: the embedder promises
  never to enter the cell again. The slot leaves the slab once no descendant's
  birth row names the cell: reclaimed if nothing reaches its storage, folded
  into the one thing that reaches it if there is exactly one, and sealed
  otherwise. `absorption` is the embedder's say over the first of those folds,
  the only one that retains more than a plain seal would
  ([liveness-matrix.md § Locality tactics](liveness-matrix.md#locality-tactics));
  it is recorded on the slot and read when the slot actually leaves.
- **`is_empty()`** asks whether the table holds nothing at all — every slot
  free, no record left in the sealed tier, and no tree cell or tombstone left
  in the pool. After a program's last release it
  is the end-of-program alarm, and the only one the substrate ships: a
  non-empty table means either a release was forgotten or a ring no merge
  dissolved survives. Naming the nodes on such a ring is a walk of the hold
  graph the crate's own tests carry, not a door on the table.

Every verb runs over the table's **scratch region**: one bump per table, reset
at the entry of `create`, `enter` and `release` and never inside one. Every
transient a verb builds lives there — the worklists a disposal cascade nests,
the runs a placement builds per operand, the views a build closure receives —
so a transient lives exactly as long as the verb that built it, and a verb on a
table whose region is already warm asks the allocator for nothing. The reset
sits at a verb's entry rather than its exit because a `release` cascades
outside any step: what the region has to be is empty when a verb starts, not
when the one before it finished. A reset runs no destructor — a bump releases
its chunks whole — so nothing with drop glue goes in the region, the same rule
the cells' own regions keep.

There are no price verbs. The substrate computes what retention costs — the
marginal price of pinning one operand into one destination, the closure a
sealed record retains, the occupancy of both tiers — but every one of those
queries is internal, along with the vocabulary they speak: the reach mask,
the sealed id, the closure and occupancy answers name nothing an embedder can
hold ([liveness-matrix.md § Bounding the two
tiers](liveness-matrix.md#bounding-the-two-tiers)). A price returns to the
embedder at exactly one place, the crossing verdict a placement consults per
operand, and it returns as one answer for one decision. The substrate ships
numbers and no threshold there; the copy-versus-hold rule over them is the
embedder's.

The substrate never runs a cell, never chooses an order, and never inspects a
continuation. It names no thread model and no async runtime: it is a
single-mutator structure, and any concurrency is the embedder's arrangement
above it.

## Passing values between cells

There is no delivery protocol. A value is always resident in a live cell's
region, transient inside an executing cell's step, or sealed with its region.
Nothing else holds a value: there is no free-standing envelope with pins of
its own. Crossing a step boundary therefore takes one of two shapes, both
built from the verbs above, and both complete through `keep` on the producing
side and `redeem` on the reading one:

- **Push.** While the producer executes, the value is minted into the
  consumer's region — or built there outright by destination-homed
  placement — and the consumer's row takes its mask. The producer keeps the
  carrier and hands the at-rest form to the embedder, which delivers it to
  the consumer; the consumer redeems it in a later step of its own. The
  producer can then die at column zero.
- **Pull.** The consumer mints a bare hold on the producer cell. The producer
  dies with a nonzero column and seals, and the consumer redeems in its own
  later step: the home resolves to the record, the consumer's hold on it is
  the entitlement, and the value comes back reaching the record's id.

Which shape an edge takes is the embedder's choice, per edge. Delivering the
at-rest carrier is the embedder's job too — the substrate ships no queue and
no mailbox, only the two doors.

### The crossing verdict

A placement over operands is where the copy-versus-pin choice is made, and
the substrate does not make it. The table is constructed with one embedder
closure, the **crossing verdict**, and consults it once per operand of every
placement — the destination-homed placement and the capturing successor store
alike, and for every operand, including one whose pin price is zero. There is
no verdict-free constructor: a table that can place a value can price the
placement.

The verdict is skipped in exactly one case, and only because there is no
choice to put: an operand homed in a tree cell, crossing to a destination
neither on that cell's chain nor under it, is a **forced copy** — nothing off
the chain may outlive the home while borrowing it, so no pin typechecks
([tree-cells.md § The one crossing rule](tree-cells.md#the-one-crossing-rule)).
Every other operand of every placement is priced and put.

What the closure sees is both halves of the price and the occupancy the
choice plays out against: the bytes a pin would *newly* keep alive, walked by
the substrate across both tiers — marginal against what the destination
already holds *and* against what the earlier operands of this same placement
have already pinned, so the first operand from a shared source carries the
shared cost and the prices sum to what the placement retains; the copy cost,
which only the embedder can
know and which it passes beside the operand; the slab's occupancy against its
cap, the sealed tier's record count and retained bytes, and the destination
region's own size. The substrate ships those numbers and no threshold.

What the closure answers decides the shape the build closure receives. A
pinned operand arrives at the destination's own region brand, so the build
may embed the borrow itself — and the mint has already folded the operand's
reach into the destination's holds. A copied operand arrives **severed**, at
a brand with no outlives relation to the destination's region, and its reach
is minted nowhere: embedding it is a compile error, so the only copy that
typechecks is a deep one through the destination's writer. Both shapes reach
the build closure out of the scratch region, and neither outlives the call:
the brands they carry are quantified over the call, so a caller has nowhere to
put a view it kept.

## What is deliberately absent

Each absence is a design statement, not a gap:

- **No acyclicity of references.** Cells may reference each other freely.
  The *hold* graph must be acyclic for a cell ever to reclaim, but the
  substrate does not enforce it: a ring keeps both cells alive forever, which
  leaks rather than dangles. A locality merge dissolves a ring it happens to
  meet — the hold on its own target has no representation — so a ring nothing
  outside holds is freed as its cells die; that is a coincidence of the
  merges' triggers, not collection. Preventing rings is the embedder's
  crossing discipline; the substrate ships no mint-time check and no ring
  detector, and `is_empty` is the alarm for the ones that survive.
- **No terminality, and therefore no error type.** Death is declared, not
  inferred, and carries no result. "Finished forever, with this value" — and
  the `Result` split between a witnessed value and a bare owned error — is a
  scheduler's terminal protocol.
- **No admission policy.** Creation refuses at the cap; a lazy generator of
  creations, a drain-first rule, or a hard failure is the layer above.
- **No frame payload.** Per-cell embedder data rides in the region, in
  captures, or in a companion cell.

## Open work

The substrate's own slices are all built; what is left is its adoption, and
the gaps nothing is scheduled against are recorded in
[the roadmap](../roadmap/README.md).

- [Rebuilding workgraph over cellgraph](../../workgraph/roadmap/adopt-cellgraph.md)
  — the first embedder's adoption.
