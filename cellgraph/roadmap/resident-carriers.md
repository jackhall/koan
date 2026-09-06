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
meet. [surface-and-duplication.md](surface-and-duplication.md) makes every
price query crate-private; this is where a price returns to the embedder, as
one closure and nothing else.

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
- *Where the closure lives — open.* A generic parameter on the table, or a
  boxed closure. Recommended: boxed; one indirect call per priced operand
  is nothing beside the walk that prices it, and it keeps every other
  signature free of the parameter.
- *Cart accretion — open.* `mark` / `absorbed_since` were handle queries the
  embedder read a loop cart's growth off. Under the one-closure rule the
  accreted bytes are either a field of the price at a crossing into the
  cart, or the closure is consulted again at the cart's release. Decide with
  adopt-cellgraph's cart criterion in hand.
- *Step-1 cost — open.* Scan each holder's residents, or keep a per-slot
  reverse index of residents naming it. Recommended: the scan; residents per
  cell are a node's fan-in, and the index can follow if the bounded-transition
  test shows growth.

## Dependencies

**Requires:**

- [Public surface and internal duplication](surface-and-duplication.md) —
  the reach vocabulary is private before a door is added over it.

**Unblocks:**

- [Rebuilding workgraph over cellgraph](../../workgraph/roadmap/adopt-cellgraph.md)
  — delivery by push or pull is this door.
