# Region-store dict values

Follows the door and pin pattern the record substrate established
([src/machine/model/values/container_substrate.rs](../../src/machine/model/values/container_substrate.rs)); terms of art are defined in
[design/value-substrates.md § Vocabulary](../../design/value-substrates.md#vocabulary).

**Problem.** A dict value's entry substrate rides the heap as an `Rc` around a
mutable-capable std map:
[`KObject::Dict(Rc<HashMap<KKey, Held>>, KType)`](../../src/machine/model/values/kobject.rs)
— a second ownership channel beside the region, and a mutation-ready structure for a
value that is frozen at construction and never written again.

**Acceptance criteria.**

- `KObject::Dict` carries `&'a DictSubstrate<'a>` — a borrow of a region-allocated
  substrate wrapper holding an immutable map frozen at construction and its
  construction memos — beside the memoized `KType`; no `Rc` in the payload.
- Dicts are born only through branded doors; the retype path shares the substrate
  borrow and swaps only the memoized `KType`.
- An escaping dict routes the seam verbs the record conversion established
  ([design/value-substrates.md § Escape](../../design/value-substrates.md#escape-pin-by-default)): total copy with exact host
  release at `Residence::Copied`, unconditional host pin on `Residence::Kept`;
  `deep_clone` is a pointer copy for the `Dict` arm.
- No runtime residence walk survives on the dict path.
- The Miri audit slate is green with region-resident dicts exercised.

**Directions.**

- *Frozen-map layout — open.* A sorted-pair slice (binary-search lookup) versus a
  hash table frozen at construction;
  [design/value-substrates.md](../../design/value-substrates.md) leaves the layout
  free. Lookup cost on the dispatch and access paths decides.

## Dependencies

**Requires:**


**Unblocks:**

- [Region-store string values](region-store-strings.md)
