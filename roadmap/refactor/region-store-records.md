# Region-store record values

**Problem.** A record value's field substrate rides the heap as an `Rc`:
[`KObject::Record(Rc<Record<Held>>, Box<Record<KType>>)`](../../src/machine/model/values/kobject.rs)
holds its field cells behind `Rc<Record<Held>>` while koan's memory model homes values
in regions ([design/memory-model.md](../../design/memory-model.md)). The `Rc` is a
second ownership regime beside the region: the residence audit walks record fields
cell-by-cell (`held_resident_in` in `kobject.rs`) instead of reading a region
residence, and the record substrate is the composite value that lift shares by
refcount rather than by region reference.

**Acceptance criteria.**

- A record value's field substrate is region-allocated; `KObject::Record` carries a
  region reference rather than an `Rc`.
- `deep_clone` and lift copy the region reference — no per-field walk, no refcount.
- The residence audit reads a record's region residence directly rather than walking
  each field cell.
- Region-stored record substrates obey the same cycle-gate and lifetime discipline as
  other region-allocated values — no new leak surface.

**Directions.**

- *Field-cell mutation under region residence — open.* Field cells are `Held`; decide
  how the region-resident substrate interacts with the retype path
  (`record_with_type`) and interior cell writes.

## Dependencies

An engine-internal memory item near
[design/memory-model.md](../../design/memory-model.md) — update it if the
region-storage families it names change. The type-side clone families adjacent to this
substrate (the field-type memo, the `alloc_ktype` re-allocation sites, the lift-path
type clones) are owned by
[Interned type content behind Copy handles](../type_memos/interned-type-content.md).

**Requires:** none — engine-internal.

**Unblocks:** none tracked yet.
