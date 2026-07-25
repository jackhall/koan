# Drop-free region death

Capstone of the project — ships the shared untyped arena of
[design/value-substrates.md § Untyped arenas](../../design/value-substrates.md#untyped-arenas-the-drop-free-end-state),
which also defines *storage family*; other terms of art are in that doc's
[§ Vocabulary](../../design/value-substrates.md#vocabulary).

**Problem.** Every region storage family is a typed sub-arena whose slots run `Drop`
at region death, even where the stored (`'static`) form owns nothing — there is no
shared untyped arena for `Drop`-free families to migrate into, so region teardown
walks slots running destructors. The residual dest-only
[`resident_in`](../../src/machine/model/values/kobject.rs) splice-free gate also
persists beside the construction doors that already enforce residence at compile
time.

**Acceptance criteria.**

- A shared untyped bump arena exists per region; every family whose stored
  (`'static`) form is `Drop`-free lives in it — the value substrates: record, list,
  and dict payloads, tagged/wrapped payload slots, strings, expression parts.
- Region death for those bytes is deallocation only — no per-slot `Drop` glue runs.
- Families designed to own things — a `Scope`'s mutable binding tables, a
  `FrameSet`'s region holds — remain typed and droppy.
- The residual dest-only `resident_in` splice-free gate is deleted (the
  reaching tier was retired in
  [residence-audit retirement](residence-audit-retirement.md)): no `resident_in`
  walk survives for composite values; residence is compile-enforced by the
  construction doors alone.
- [design/memory-model.md](../../design/memory-model.md)'s storage-family and
  move-in-audit prose matches the shipped model, reconciled with
  [design/value-substrates.md](../../design/value-substrates.md).
- The Miri audit slate is green across the converted families.

**Directions.**

- *Arena granularity — open.* One untyped bump arena per region versus per-family
  segments inside it; alignment handling and debuggability decide.

## Dependencies

**Requires:**

- [Region-store expression parts](region-store-expressions.md) — the last substrate
  conversion; every value family must be `Drop`-free in stored form before the move.
- [Residence-audit retirement](residence-audit-retirement.md) — the composite
  residence tiers this item deletes are dispositioned per-site there first.
- [Binding tables as witnessed carriers](binding-tables-witnessed-carriers.md) —
  removes the `data` table's typed-and-droppy residue, so its entries can migrate
  into the untyped bump arena.

**Unblocks:** none tracked yet.
