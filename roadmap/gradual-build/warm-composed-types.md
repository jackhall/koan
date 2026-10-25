# Warm composed types

The types a settlement under a composition derives, kept warm across hops.

**Problem.** A settlement under a composition rests no type
([types across the arena](../../src/build/README.md#types-across-the-arena)),
so a type its facts derive is a term
over the key's handles that the frame mints into its own tiers once per
binding event. A `Shares` frame keeps it warm through its host's scratch
alone, and a `Fresh` loop calling one composed callable under one binding
mints every derived type again each hop, as a run-content binding does.

**Acceptance criteria.**

- A `Fresh` loop calling one composed callable under one binding mints each
  type its facts derive once, not once per hop, and what it keeps warm lies
  under the arena's budget.
- No arena node holds a type.

**Directions.**

- *Where a derived type waits — open.* A memo in the frame's activation keyed
  on the binding event, which a hop's hand-off carries; or a tier the
  settlement under the composition owns, evicted with it, which revisits the
  rule that no arena node holds a type; or the composition's facts naming the
  derived term once, so a frame mints it from the term in O(1).

## Dependencies

**Requires:** none.

**Unblocks:** none.
