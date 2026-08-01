# Drop-free region death

Capstone of the project — ships the shared untyped arena of
[design/value-substrates.md § Untyped arenas](../../design/value-substrates.md#untyped-arenas-the-drop-free-end-state),
which also defines *storage family*; other terms of art are in that doc's
[§ Vocabulary](../../design/value-substrates.md#vocabulary).

**Problem.** The value-substrate families still held in typed sub-arenas — record, list
and dict payloads, tagged/wrapped payload slots — run `Drop` at region death even though
their stored (`'static`) form owns nothing, so teardown walks slots running destructors.
A substrate's **index** metadata is droppy for a second reason: a dict's key→index
`hashbrown` table and a record's field-name table
([`ContainerSubstrate`](../../src/machine/model/values/container_substrate.rs)) are
default-`Global` heap allocations the substrate owns, and a record's field names are
owned `String`s besides — `Record<V>` is shared with the type registry, where owned keys
are correct, so the conversion is a change of index representation rather than of the
value family. Strings and expression parts already live in the bump and are not in that
residue; operator groups reach it under their own item. The composite arms of the
dest-only [`resident_in_visiting`](../../src/machine/model/values/kobject.rs) walk also
persist beside the construction doors that already enforce residence at compile time.

**Acceptance criteria.**

- Every family whose stored (`'static`) form is `Drop`-free lives in the shared
  per-region bump: the remaining value substrates — record, list and dict payloads,
  tagged/wrapped payload slots — join the strings, expression parts and operator groups
  already hosted there, and no typed sub-arena holds a `Drop`-free family.
- A substrate's index metadata is bump-hosted too: a dict's key→index table and a
  record's field-name table are `Copy` bump-hosted structures (a sorted slice of
  `(&'a str, usize)` entries for the name-keyed ones), so no substrate owns a heap
  allocation of its own.
- Region death for those bytes is deallocation only — no per-slot `Drop` glue runs.
- Families designed to own things — a `FrameSet`'s region holds — remain typed
  and droppy.
- The dest-only `resident_in_visiting` walk is deleted — the last runtime
  residence check over a composite value
  ([design/witness-hosting.md § Residence enforcement](../../design/witness-hosting.md#residence-enforcement)):
  no residence walk survives for composite values; residence is compile-enforced
  by the construction doors alone.
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

**Requires:** none — the composite residence tiers this item's walk-deletion
depends on are already retired.

**Unblocks:** none tracked yet.
