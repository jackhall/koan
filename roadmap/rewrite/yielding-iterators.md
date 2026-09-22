# Yielding iterators

Streams whose elements are produced one demand at a time.

**Problem.** A koan computation that produces a sequence materializes the whole
sequence before any consumer sees an element. [values](../../src/values/README.md)
has no shape for "an element, and the rest still to compute", and a call
delivers exactly one result, so there is no surface for a producer whose
elements a consumer takes lazily.

**Acceptance criteria.**

- A koan surface yields one element together with the tail call that produces
  the rest of the stream, and a delegating form yields every element of a
  sub-stream before continuing with a tail call.
- A stream at rest is a value — a bound call (a function value and its evaluated
  arguments), or a stack of pending bound calls under delegation — so it
  occupies no cell and no slab slot, and a stream nothing demands is reclaimed
  with the region it rests in.
- Demanding an element is an ordinary dispatch: the producing call runs in a
  cell the scheduler queues like any other, and places the element and the rest
  of the stream into the consumer's frame.
- Demanding the same stream value twice runs its tail twice.
- An unbounded stream whose tails are named functions, consumed one element at a
  time by a tail-recursive loop, runs in memory bounded by its delegation depth
  and independent of how many elements it has produced.

**Directions.**

- *The tail rides the yield — decided.* A yield is a function's terminal: the
  element plus the tail call that continues the stream. A yield in the middle of
  a block is written by moving the rest of the block into a named function whose
  parameters are the locals it reads; a yield reached through a non-tail call is
  what delegation covers.
- *Delegation over continuation-passing — decided.* A closure retains what it
  captures, and a callable copies only by re-tying its whole knot
  ([src/knot/README.md](../../src/knot/README.md#weight-and-copy)), so a continuation-passing stream
  either pins the producing call's storage into the consumer on every element
  or copies every capture per element, and a tree-cell loop hop cannot carry
  the pin across a tail call at all. Delegation keeps pending tails as data:
  bound calls whose function values rest in an enclosing scope, pinned at zero
  marginal price, and whose arguments copy.
- *A stream at rest is a value, not a sleeping cell — decided.* A region
  releases whole and runs no destructor, so nothing signals when the last handle
  to a sleeping producer cell dies; a producer under another root delivers every
  producer-born part by forced copy
  ([tree cells](../../cellgraph/src/tree/README.md)); and flat memory would take
  a fresh slab cell per element. With the tail fused into the yield, nothing
  mid-block stays suspended between demands.
- *Re-runnable streams — decided.* A stream is a persistent value; one-shot use
  is a restriction a later layer may add.
- *Tail arguments evaluate at the yield — decided.* A bound call holds evaluated
  arguments; a tail evaluated at the next demand would be a closure, with
  continuation-passing's retention.
- *Yield crossings take the ordinary verdict — decided.* A yield's element and
  rest cross into the consumer's frame as a return value does, priced by
  [values' verdict](../../src/values/README.md#crossing), with no rule of their
  own.
- *A finished sub-stream's value — decided, as an optional feature.* `Done` may
  carry a value that delegation passes to the continuing tail as one more
  argument, as Python's `yield from` returns its sub-generator's result. The
  acceptance criteria do not require it.
- *Surface spelling — open.* `YIELD … THEN` and `YIELD FROM … THEN` are
  placeholders; no keywords are chosen.
- *Pending-tail representation — open.* A nested `Concat(sub, tail)` re-wraps
  each element through one cell per nesting level; a flattened stack of bound
  calls makes a demand one step and one child, and copies the O(depth) stack
  across each consumer hop.
- *A bound call's shape — open.* A tagged record over a function value, or a
  node of the callable parameter [`knot`](../../src/knot/README.md) closes,
  beside the function node.
- *Buffered channels — deferred.* A policy layer for buffering and
  multi-producer merge, designed once streams ship.

## Dependencies

**Requires:**

- [Dispatch](dispatch.md) — a demand for an element is an ordinary dispatch.

**Unblocks:**

- [Destination-homed construction](../old_foundation/destination-homed-construction.md) — carried edge; its demand-edge premise predates `alloc_into` and needs re-checking.
- [Retire the old runtime](retire-the-old-runtime.md) — the last of the execution surface `workgraph`'s DAG layer still owns.
