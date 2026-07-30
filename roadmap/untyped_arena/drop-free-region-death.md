# Drop-free region death

Capstone of the project — ships the shared untyped arena of
[design/value-substrates.md § Untyped arenas](../../design/value-substrates.md#untyped-arenas-the-drop-free-end-state),
which also defines *storage family*; other terms of art are in that doc's
[§ Vocabulary](../../design/value-substrates.md#vocabulary).

**Problem.** The value-substrate families still held in typed sub-arenas — record, list
and dict payloads, tagged/wrapped payload slots — run `Drop` at region death even though
their stored (`'static`) form owns nothing, so teardown walks slots running destructors.
Strings, expression parts and operator groups reach the bump under their own items and
are not in that residue. The residual dest-only
[`resident_in_visiting`](../../src/machine/model/values/kobject.rs) splice-free gate
also persists beside the construction doors that already enforce residence at
compile time.

**Acceptance criteria.**

- Every family whose stored (`'static`) form is `Drop`-free lives in the shared
  per-region bump: the remaining value substrates — record, list and dict payloads,
  tagged/wrapped payload slots — join the strings, expression parts and operator groups
  already hosted there, and no typed sub-arena holds a `Drop`-free family.
- Region death for those bytes is deallocation only — no per-slot `Drop` glue runs.
- Families designed to own things — a `FrameSet`'s region holds — remain typed
  and droppy.
- The residual dest-only `resident_in_visiting` splice-free gate is deleted (the
  reaching tier was retired in
  [residence-audit retirement](residence-audit-retirement.md)): no residence
  walk survives for composite values; residence is compile-enforced by the
  construction doors alone.
- [design/memory-model.md](../../design/memory-model.md)'s storage-family and
  move-in-audit prose matches the shipped model, reconciled with
  [design/value-substrates.md](../../design/value-substrates.md).
- The Miri audit slate is green across the converted families.

**Directions.**

- *Arena granularity — decided.* One bump per region
  ([`Region::bump`](../../workgraph/src/witnessed/region.rs)), shared with the
  container metadata already living there — the reach-run partitions and cell index
  blocks a sectioned container names. No per-family segments: the only occupancy
  figure anyone reads is the region's total live bytes, reported by
  [`Region::bump_bytes`](../../workgraph/src/witnessed/region.rs), which the
  copy-versus-pin decision weighs against a candidate value's own copy size.

## Dependencies

**Requires:**

- [Region-store expression parts](region-store-expressions.md) — the last substrate
  conversion; every value family must be `Drop`-free in stored form before the move.
- [Residence-audit retirement](residence-audit-retirement.md) — the composite
  residence tiers this item deletes are dispositioned per-site there first.

**Unblocks:** none tracked yet.
