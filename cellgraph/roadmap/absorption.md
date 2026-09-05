# Absorption

**Problem.** Every cell that dies uniquely held mints a count-1 sealed record,
and every chain of single-consumer producers becomes a chain of such records:
one id, one index entry, one accessor indirection each. The three merges of
[liveness-matrix.md § Locality tactics](../design/liveness-matrix.md#locality-tactics)
— death-time absorption into a unique live holder, seal-time absorption of a
count-1 sealed region into its sealing holder, and seal-into-namer for a
column-zero cell with a single sealed namer — are unimplemented, so the
degenerate cases the model is designed around stay rare rather than common.

**Acceptance criteria.**

- A cell released with `column == bit(M)` and an empty naming set splices
  its chunks onto M's region, ORs its row into M's through the standard mint,
  rewrites M's stored masks naming the dead slot to nothing, and mints no
  sealed record; reads of the absorbed values stay on the per-value-mask
  path.
- At a cell's seal, each count-1 sealed region in its hold set is absorbed:
  aggregates OR, chunks splice, reverse-naming entries repoint, and the
  absorbed id is released.
- A cell released with a zero column and a singleton naming set seals into that
  namer instead of minting a record.
- Absorption of already-sealed storage into a live region never happens; a
  test constructs the shape and observes a seal instead.
- Death-time absorption is refusable per release by the embedder, so a
  priced choice can decline it; the default accepts.
- The property test of the sealed tier extends to interleavings that trigger
  each merge, and the mask-validity assertion still holds.

**Directions.**

- *Refusal signal — decided.* An enum parameter on `release`:
  `Absorption::IntoHolder` (the default an embedder passes when it has no
  price to weigh) or `Absorption::Refused`. The choice is recorded on the
  slot and applied when the slot disposes, which may be later than the
  release. The two sealed-tier merges are not refusable: they retain exactly
  what a plain seal retains.
- *Rings — decided.* A merge whose source holds its target runs anyway; the
  hold becomes a self-hold and vanishes, and a record left with no holders is
  reclaimed on the spot. A ring whose cells die one at a time with no outside
  holder is freed rather than leaked; rings that survive still leak and the
  debug detector still names them.
- *Group sealing — open.* Delimiting a dying subtree that seals as one
  record is design work; the item ships the three merges without it and
  records the group case as open in the design doc if it is not resolved
  here.

## Dependencies

**Requires:** none — the cell substrate it extends has shipped.

**Unblocks:**

- [Retention pricing](retention-pricing.md) — the priced choice decides when
  to absorb.
