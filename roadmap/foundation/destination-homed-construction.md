# Destination-homed construction

Implement the memory primitive pinned in
[design/destination-homed-construction.md](../../design/destination-homed-construction.md):
a marked node constructs its result directly in a destination region other
than its own.

**Problem.** Every value is constructed in its producer's incarnation region;
crossing into a longer-lived destination pays a structural copy at the
delivery adopt (`Scope::adopt_for_binding`, the delivery walk of
[design/memory-model.md](../../design/memory-model.md)) even when the
destination is knowable before construction — a yield operand's demanding
consumer, a callee result's caller frame. The copy is pure overhead for
values that provably end up at the destination.

**Acceptance criteria.**

- A node in a delivering position constructs its result value in the
  destination region, with no delivery-time copy for the parts built there.
- The mark propagates along result positions only — block tails, arm
  results, call boundaries; a value bound to a local before delivery is
  unmarked and crosses by ordinary adoption.
- A step's allocation into a foreign region goes through a library-boundary
  grant whose discipline preserves the run-then-apply soundness argument of
  [design/tail-call-optimization.md](../../design/tail-call-optimization.md).
- The yield client works end to end: a generator's yield operand is built in
  the demanding consumer's region, and a filtering generator's rejected
  candidates never reach it.

**Directions.**

- *Foreign-region grant — open.* What hands a step an allocating brand for a
  region it does not own, and the grant's lifetime discipline across the
  read-producer/write-destination aliasing window.
- *Mark carrier across calls — open.* A region-free (slot, edge) name
  resolved at construction time, or a dedicated region-ful channel with its
  own retention story; the kept-first contract channel is region-free by
  design and cannot carry it as-is.
- *Fan-out posture — open.* Decline the mark when a terminal's destination
  set may still grow, or pick a primary destination and copy to the rest.
- *Client sequencing — decided* per
  [design/destination-homed-construction.md](../../design/destination-homed-construction.md):
  yields first, returns/bindings second, loop-carried arguments only if the
  TCO constraints resolve cleanly.

## Dependencies

**Requires:**

- [yielding-iterators.md](yielding-iterators.md) — the first client; the
  demand edge supplies the destination this primitive resolves.

**Unblocks:** none tracked yet.
