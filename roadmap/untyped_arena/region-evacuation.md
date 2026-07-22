# Region evacuation at frame death

Applies the copy-versus-pin pricing of [cost-driven-copy](cost-driven-copy.md)
to a whole region at once; terms of art are defined in
[design/value-substrates.md § Vocabulary](../../design/value-substrates.md#vocabulary).

**Problem.** The relocation seam prices copy against pin per value, at the moment
it crosses ([cost-driven-copy](cost-driven-copy.md)): each decision sees one
escapee in isolation, against the region's allocated tally *at crossing time*. No
decision runs at the one point where the full picture exists — frame death, where
the region's final allocated total and its complete survivor set (every value in
the region that a consumer still holds pinned) are both known.
A value that pinned mid-call (a large fraction of the region *so far*) stays a
pin even when the finished frame dwarfs it, so the consumer retains the whole
region — result and temporaries — until its own scope releases the reach.

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
- The Miri audit slate exercises both outcomes at frame death.

**Directions.**

- *Evacuation's ratio constant — open.* Reuse the per-crossing seam's α or give
  frame death its own tuning constant; the retention profiles differ (a crossing
  pins prospectively, frame death retires a known-dead region).

## Dependencies

**Requires:**

- [Cost-driven copy at the escape seam](cost-driven-copy.md) — supplies the copy
  verb, the memoized costs, and the ratio rule this site applies to the survivor
  set.

**Unblocks:** none tracked yet.
