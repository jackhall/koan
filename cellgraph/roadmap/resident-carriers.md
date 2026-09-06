# Resident carriers and the crossing price

A value cannot reach a second step, and a crossing has no price.

**Problem.** The placement doors hand back a carrier branded to the step that
built it, so a value built into a consumer's region by `alloc_into` is
unreachable from the consumer's own later step, and a value a consumer holds
its producer for has no read door at all. Neither the push nor the pull shape
of [cellgraph.md § Passing values between
cells](../design/cellgraph.md#passing-values-between-cells) can complete; the
one value that crosses steps is the continuation, through its own slot. The
copy-versus-pin choice at a crossing has no price either: the embedder knows
what a copy costs, the table knows what a pin retains, and no door lets the two
meet. Every price query is crate-private ([liveness-matrix.md § Bounding the
two tiers](../design/liveness-matrix.md#bounding-the-two-tiers)); this is where
a price returns to the embedder, as one closure and nothing else.

**Acceptance criteria.**

- A third carrier state exists, **at rest**: lifetime-free, `Copy` when the
  family's erased form is, opaque. It is born only from a placement door or
  from an in-step carrier, and carries no reach of its own: the table records
  what each resident contains in a per-cell table the seal transition
  rewrites, and the at-rest carrier names its entry by a private key. A value
  and its reach cannot be paired by the embedder because nothing is
  pairable.
- One door on the step context redeems an at-rest carrier into an in-step
  carrier. It refuses, as an error result, unless the executing cell is the
  carrier's home, holds its home, or holds the record its home sealed into. A
  redeemed value whose home sealed comes back with reach derived from the
  record.
- Push and pull both complete, and a test shows each: a value built into a
  consumer from a producer's step is read in the consumer's step; a value a
  consumer holds its producer for is read after the producer seals.
- The continuation is a resident like any other: one stored-mask path.
- The table takes one embedder closure at construction, the **crossing
  verdict**. A placement over operands consults it per operand with both
  prices — the bytes a pin would keep alive across both tiers and the tier
  occupancy, from the table; the copy cost, passed by the embedder with the
  operand — and hands the build closure each operand in a form that permits
  only the verdict it got: embeddable at the region brand on pin, readable
  at a shorter brand on copy. No type outside the closure's parameter names
  a price.
- The seal transition's holder rewrite is bounded by the holders' resident
  counts, and the bounded-transition test asserts it.
- [cellgraph.md](../design/cellgraph.md) and
  [liveness-matrix.md](../design/liveness-matrix.md) describe the three
  carrier states, the redeem door, and the verdict closure.

**Directions.**

- *Where a resident's reach lives — decided.* In the table, one mask per
  resident, rewritten at seal step 1 exactly as the continuation's is today.
  Not in the carrier (the seal could not rewrite it) and not as the home
  cell's whole hold set (a node that forwards one of several deps would pin
  them all).
- *Entitlement to redeem — decided.* Home is the executing cell, or the
  executing cell holds the home, or holds the record the home sealed into.
  Anything else is an error, never a panic.
- *The verdict's inputs — decided.* Both sides. The embedder passes its copy
  cost as a plain number beside the operand; the table supplies what a pin
  retains and how full the tiers are.
- *Where the closure lives — decided.* Boxed, taken by the table's
  constructor; there is no verdict-free constructor. One indirect call per
  priced operand is nothing beside the walk that prices it, and every other
  signature stays free of the parameter.
- *What a redeemed value reaches once its home sealed — decided.* The
  record's id alone, not the id plus its aggregate. A hold on the record
  keeps its aggregate alive transitively, so the id covers; it adds no
  direct edges to the destination's row, so what the record reaches still
  merges into it; and a mask naming only an id has no slab bit to go stale
  when the value is kept again under the redeeming cell.
- *What the pin price counts — decided.* What pinning the operand would
  newly retain: the walk from the operand's reach with the destination, its
  pin row, and its sealed-hold set already marked as seen, across both
  tiers. An operand homed or directly held in the destination prices at
  zero; a node held only through a directly held node is still billed, so
  the figure only ever over-bills toward copying.
- *Which holds entitle a redeem — decided.* The pin row and the birth row
  both: each keeps the home in the slab with its storage intact, and the
  parent chain is what koan's lexical lookup walks.
- *Which placements consult the verdict — decided.* Both of them: the
  destination-homed placement and the capturing successor store take the
  same priced operands and hand the same pinned-or-copied views. Every
  operand is consulted, including one whose pin price is zero.
- *Loop shape and cart accretion — decided.* A loop is three cells: two
  per-hop cells that alternate, and one longer-lived storage cell. A hop
  builds the next hop's arguments into the next hop's cell and its results
  into the storage cell, so the retiring hop dies at column zero and is
  freed, never absorbed. The storage cell accretes only its own replaced
  values, so the consolidation signal is its size, carried on the crossing
  as the destination's bytes; there is no absorbed-bytes figure, and the
  mark / absorbed-since queries go.
- *Step-1 cost — decided.* Scan each holder's residents; residents per cell
  are a node's fan-in. A per-slot reverse index can follow if the
  bounded-transition test shows growth.

## Dependencies

**Requires:** none — the reach vocabulary the door is built over is already
crate-private, and the price queries it consumes are in the crate awaiting a
consumer.

**Unblocks:**

- [Rebuilding workgraph over cellgraph](../../workgraph/roadmap/adopt-cellgraph.md)
  — delivery by push or pull is this door.
