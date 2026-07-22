# Cost-driven copy at the escape seam

Implements [design/value-substrates.md § Cost-driven copy](../../design/value-substrates.md#cost-driven-copy-the-optimization);
terms of art are defined in that doc's
[§ Vocabulary](../../design/value-substrates.md#vocabulary).

**Problem.** Pin-by-default escape
([design/value-substrates.md § Escape](../../design/value-substrates.md#escape-pin-by-default))
retains the producer's whole region — the result *and* the call's temporaries —
until the consumer's scope releases the reach. Nothing bounds that retention: the
relocation seam ([`transfer_into`](../../workgraph/src/witnessed/delivered.rs) and
its [`copy_carried`](../../src/machine/execute/lift.rs) hook) has no copy verb that
rebuilds a value at the destination and releases the producer pin, and no memoized
cost exists to price such a copy against the pin.

**Acceptance criteria.**

- Every composite substrate memoizes at construction, in the same pass that computes
  the type join: its **copy cost** (leaves contribute weight — cell count, with
  strings byte-weighted; nested substrates contribute their memoized cost; borrow
  leaves contribute zero) and a **contains-borrows bit** (whether any transitive
  cell is a closure or module borrow).
- The relocation seam chooses per value in O(1) from the memoized copy cost and the
  region's allocated total: **copy** — a total rebuild of the value's reachable
  structure at the destination brand, releasing the producer pin — when
  `copy_cost < α × region_allocated` (a set contains-borrows bit discounts the
  payoff); **pin** otherwise. No partial spine copy exists — a partial copy would
  pay the copy *and* keep the pin.
- The policy is semantically invisible: a program's observable behavior is identical
  under forced-copy and forced-pin.
- The Miri audit slate exercises both verbs at the seam.

**Directions.**

- *Where the memo rides — decided.* On the substrate wrapper, beside the
  contains-borrows bit ([region-store-records](region-store-records.md) sets the
  pattern); carriers stay lean.
- *Contains-borrows treatment — decided* per
  [design/value-substrates.md § Cost-driven copy](../../design/value-substrates.md#cost-driven-copy-the-optimization):
  the bit is an optimization gate, never a soundness input — the copy pass computes
  exact release per retiring host via address-table checks on surviving borrow
  leaves, so a foreign-leaf value still releases its home.
- *Set-bit weighting — open.* Whether a set bit skips the copy attempt outright or
  only discounts the ratio; pick from measurement once the ratio exists.
- *α — open.* A tuning constant of the seam, not observable in language semantics;
  pick from measurement once both verbs exist.

## Dependencies

**Requires:**

- [Region-store record values](region-store-records.md) — the first pinned
  substrate; the memo lands in its construction pass.

**Unblocks:**

- [Region evacuation at frame death](region-evacuation.md)
