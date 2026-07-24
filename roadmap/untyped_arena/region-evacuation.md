# Region evacuation at frame death

Applies the copy-versus-pin pricing of
[design/value-substrates.md § Cost-driven copy](../../design/value-substrates.md#cost-driven-copy-the-optimization)
to a whole region at once; terms of art are defined in
[design/value-substrates.md § Vocabulary](../../design/value-substrates.md#vocabulary).

**Problem.** The relocation seam prices copy against pin per value, at the moment
it crosses
([design/value-substrates.md § Cost-driven copy](../../design/value-substrates.md#cost-driven-copy-the-optimization)):
each decision sees one
escapee in isolation, against the region's allocated tally *at crossing time*. No
decision runs at the one point where the full picture exists — frame death, where
the region's final allocated total and its complete survivor set (every value in
the region that a consumer still holds pinned) are both known.
A value that pinned mid-call (a large fraction of the region *so far*) stays a
pin even when the finished frame dwarfs it, so the consumer retains the whole
region — result and temporaries — until its own scope releases the reach.

The same per-value blindness caps a second consumer of the pricing: the bind seam
([`Scope::copy_delivered_substrate`](../../src/machine/core/scope/reach.rs)) prices only
top-level records; every other substrate carrier (`List` / `Dict` / `Tagged` / `Wrapped`)
copies unconditionally. A bind-lifetime pin retains its producer region for the whole life
of the binding, so pricing the loop-carried carriers to pin would chain one retired per-hop
region per iteration across a tail loop's `it` binds — O(depth) live regions where a copy
holds O(1). The escape seam ([`seam_verb`](../../src/machine/execute/lift.rs)) already prices
every carrier, so the bind seam's records-only line is a heuristic guarding tail-region
turnover, not a soundness boundary; nothing lifts it until a retiring region's fate is
decided as a whole.

**Acceptance criteria.**

- Frame finalization with escapees decides the region's fate from memoized
  numbers alone — the survivor set's summed copy cost against the region's final
  allocated total — with no liveness walk and no forwarding map.
- **Evacuate**: when the summed survivor cost is below the seam's ratio of the
  final allocated total and no survivor's contains-borrows bit leans the decision
  away, every survivor is rebuilt at its consumer's brand and the region
  deallocates at frame death.
- **Transfer** is the default otherwise: survivors keep their borrows and the
  region rides the frame-retention hold, per
  [design/value-substrates.md § Escape](../../design/value-substrates.md#escape-pin-by-default).
- Evacuation is all-or-nothing per region: the region deallocates only when every
  survivor leaves — no partial evacuation that pays the copy and keeps the pin.
- The choice is semantically invisible: a program's observable behavior is
  identical whether its regions were evacuated or transferred.
- The bind seam ([`Scope::copy_delivered_substrate`](../../src/machine/core/scope/reach.rs))
  prices every substrate carrier through the cost chooser, not just top-level records: a
  loop-carried pin holds O(1) live regions regardless of depth, because evacuation
  deallocates each retired per-hop region rather than letting a bind-lifetime pin chain them.
- The Miri audit slate exercises both outcomes at frame death.

**Directions.**

- *Evacuation's ratio constant — open.* Reuse the per-crossing seam's α or give
  frame death its own tuning constant; the retention profiles differ (a crossing
  pins prospectively, frame death retires a known-dead region).

## Dependencies

**Requires:**

- [Reach ownership split](reach-split.md) — droppable pins, so pricing
  loop-carried carriers to pin does not chain retired regions across a tail loop.

**Unblocks:** none tracked yet.
