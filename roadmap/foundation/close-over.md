# CLOSE OVER

**Problem.** A closure's captures carry reach: the closure value pins every
region its borrows lead to, and the pinning chains — a closure over a closure
retains the whole cactus of regions beneath it. Koan has no surface that
severs this: there is no way to build a closure whose captures are copied
rather than pinned, so a closure's retention footprint is dictated entirely by
where its captured values happen to live.

**Acceptance criteria.**

- A koan form builds a closure whose capture set is copied into the closure's
  own region; the built value holds no reach into any region it was captured
  from, and a test observes that emptiness through the memory substrate.
- The copy is transitive — everything the captures' borrows lead to is copied
  — since a shallow copy leaves borrows pointing where they pointed and severs
  nothing.

**Directions.**

- *Surface or policy — open.* An explicit builtin, an automatic policy at
  closure creation, or both (a heuristic default with an explicit override).
  If any capturable value has observable identity, the copy changes meaning
  and the explicit form must be a deliberate user choice.
- *When the copy pays — open.* The reach evidence prices the operation before
  it runs (empty reach means nothing to do; each pinned region names storage
  to pull from), but the copy-or-pin decision is expected to be more complex
  than a size heuristic.

## Dependencies

The aspirational liveness-matrix design leans on this form as its proactive
consolidation lever
([liveness-matrix.md](../../workgraph/design/liveness-matrix.md)).

**Requires:** none — foundation.

**Unblocks:** none tracked yet.
