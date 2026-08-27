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

**Directions.**

- *Surface form — open.* No syntax is proposed yet; enumerate options with the
  user.
- *Node lifecycle — open.* How a node yields without finalizing: a yield
  verdict alongside the drain's `StepVerdict`, a node that re-arms after
  delivery, or a dedicated node kind.
- *Pacing interface — open.* The same shape as scheduler-controlled node
  creation — the embedder hands the scheduler an iterator and the scheduler
  consumes it against its resources
  ([liveness-matrix.md](../../workgraph/design/liveness-matrix.md)) — but
  whether the two share one interface is undecided.

## Dependencies

**Requires:** none — foundation.

**Unblocks:** none tracked yet.
