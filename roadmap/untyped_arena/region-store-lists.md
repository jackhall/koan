# Region-store list values

Follows the door and pin pattern established by
[region-store-records](region-store-records.md); terms of art are defined in
[design/value-substrates.md § Vocabulary](../../design/value-substrates.md#vocabulary).

**Problem.** A list value's element substrate rides the heap as an `Rc`:
[`KObject::List(Rc<Vec<Held>>, KType)`](../../src/machine/model/values/kobject.rs)
holds its element cells behind `Rc<Vec<Held>>` — a second ownership channel beside
the region, shared by refcount on lift and walked cell-by-cell by the move-in
residence audit.

**Acceptance criteria.**

- `KObject::List` carries `&'a ListSubstrate<'a>` — a borrow of a region-allocated
  substrate wrapper holding the arena element slice and its construction memos —
  beside the memoized `KType`; no `Rc` in the payload.
- Lists are born only through branded doors; the retype path shares the substrate
  borrow and swaps only the memoized `KType`.
- An escaping list routes the seam verbs established by
  [region-store-records](region-store-records.md): total copy with exact host
  release at `Residence::Copied`, unconditional host pin on `Residence::Kept`;
  `deep_clone` is a pointer copy for the `List` arm.
- No runtime residence walk survives on the list path.
- The Miri audit slate is green with region-resident lists exercised.

## Dependencies

**Requires:**

- [Region-store record values](region-store-records.md) — establishes the door and
  pin pattern this conversion follows.

**Unblocks:**

- [Region-store string values](region-store-strings.md)
