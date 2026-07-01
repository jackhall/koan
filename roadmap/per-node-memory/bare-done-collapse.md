# Bare-`Done` terminal collapse

**Problem.** [`finalize_terminal`](../../src/machine/execute/finalize.rs) wraps a live `Carried` as
`Witnessed::new(checked, witness)` under a separately-computed `dep_reached ∪ producer` set. Every node
whose step is `NodeStep::Done` (not `DoneWitnessed`) routes here, so this asserted bundle is the witness
point for the entire non-construction terminal class. The `dep_reached: FrameSet` threading and the
`NodeStep::Done` / `DoneWitnessed` split exist only to feed the assertion — the reach is recomputed
beside the value rather than read off the delivered dep carriers.

**Acceptance criteria.**

- Every node finalizes a witnessed carrier: a region-pure result through the empty-set `resident` path
  (its producing frame folded in at close), a dep-reaching result by folding its delivered dep carriers —
  so `finalize_terminal`'s asserted `Witnessed::new`, the `dep_reached: FrameSet` threading, and the
  `NodeStep::Done` / `DoneWitnessed` split collapse to one witnessed terminal. This is the region-pure
  `resident` / empty-set birth of
  [§Construction](../../design/per-node-memory.md#construction-yoke-merge-map-and-one-wrapper-per-node)
  and the delivered-dep-carrier fold of
  [§Storage and access](../../design/per-node-memory.md#storage-and-access-seal-open-transfer_into),
  now covering the terminal class too.

**Directions.**

- *Bare-`Done` collapse — decided.* Bare-terminal producers deliver witnessed carriers (region-pure →
  empty-set `resident`, the producer frame folded at close; dep-reaching → fold the delivered dep
  carriers via `transfer_into` / `merge`), retiring `finalize_terminal` and `NodeStep::Done` and
  replacing the `dep_reached` threading with dep-carrier folding.

## Dependencies

**Requires:**

- [The honest single-region witness substrate](../../src/witnessed.rs) — the `resident` / fold path seals on the honest witness surface.

**Unblocks:**

- [Witnessed type and region operands](type-operand-carriers.md) — the capstone's `Witnessed::new` deletion needs this item's bare-`Done` caller retired.
