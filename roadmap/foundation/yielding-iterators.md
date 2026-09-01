# Yielding iterators

**Problem.** A workgraph node produces exactly one value: delivery happens at
finalize, and the slot reclaim behind it is unconditional
([dag-scheduler.md](../../workgraph/design/dag-scheduler.md)). A koan
computation that produces a sequence must therefore materialize the whole
sequence before any consumer sees an element — there is no surface for a
producer that yields values incrementally, and no node lifecycle that emits
more than once before dying.

**Acceptance criteria.**

- A koan surface exists for an iterator whose elements are produced one at a
  time by a live producer and consumed lazily.
- The backing node yields many values before dying — a yield delivers an
  element without finalizing the node.
- The scheduler decides when the producer runs next: production is paced by
  the scheduler's resources, not forced by the producer.
- A generator's frame memory turns over per loop iteration — an unbounded
  stream consumed and dropped element-by-element runs in constant memory.
- An undemanded dormant producer is reclaimed when its last outbound edge is
  released, and dormant slots do not trip the drain-end deadlock census.
- The pin ring between a persistent producer frame and its consumer is
  structurally unconstructible, not merely detectable by the debug pin-ring
  audit ([reach.md § Debug audits](../../workgraph/design/reach.md#debug-audits)).

**Directions.**

- *Node lifecycle — decided.* A yield is a **delivering park**: the parked
  node delivers a value on the way down
  ([continuations.md](../../design/execution/continuations.md)), and waits in
  a new *dormant* resting state — nothing pending, not enqueued — woken by
  the install door's park branch when a consumer wires an edge to it.
  Nothing retires at a yield; the frame region dies at the recursive tail
  call, one region per loop iteration.
- *Pacing — decided.* A resume is a demand: consumers mint demand edges at
  the moment they ask, and a woken slot waits its turn in the ordinary
  queues, so the scheduler's resources pace production.
- *Element crossing — decided* per
  [design/destination-homed-construction.md](../../design/destination-homed-construction.md):
  the anti-ring crossing rule (producer-born parts copy; references upward
  and sideways cross free), with destination-homed construction of the yield
  operand as its zero-copy tier.
- *Crossing-rule enforcement scope — open.* At yield deliveries only, or at
  every adopt language-wide (making pin rings unconstructible everywhere and
  the liveness matrix's hold-graph acyclicity an enforced invariant).
- *Surface form — open.* Generator syntax vs. stream constructors; no syntax
  proposed yet — enumerate options with the user.
- *Stored stream handle — open.* What a stored handle holds between demands,
  and what an escaped handle means when the generator's owning frame dies.
- *Dormant-slot matrix habitat — open.* A dormant slot's frame is
  live-but-not-executing; the liveness matrix names no habitat for any
  installed-but-unrun frame
  ([liveness-matrix.md](../../workgraph/design/liveness-matrix.md)) — the
  gap predates this item and needs one answer covering both.
- *Buffered channels — deferred.* A policy layer over dormant producers (a
  buffer is a window policy; per-message release is whole-region death of
  tiny per-message regions) plus one new mechanism, disjunctive wake for
  multi-producer merge. Ships after the iterator substrate.

## Dependencies

**Requires:** none — foundation.

**Unblocks:**

- [destination-homed-construction.md](destination-homed-construction.md) —
  the demand edge supplies the destination the primitive's first client
  resolves.
