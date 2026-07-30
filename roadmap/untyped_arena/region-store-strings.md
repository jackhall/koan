# Region-store string values

Terms of art are defined in
[design/value-substrates.md § Vocabulary](../../design/value-substrates.md#vocabulary).

**Problem.** String bytes are owned wherever they appear in the value family:
`KObject::KString(String)`, the `Tagged { tag: String, .. }` discriminant, and
`KKey::String(String)` dict keys ([kkey.rs](../../src/machine/model/values/kkey.rs))
each own a heap allocation, so `deep_clone` copies bytes for the `KString` arm and
every value-family slot that can hold a string carries `Drop` glue — blocking the
untyped-arena move for the whole family. No region storage hosts bare bytes: a typed
family cell's slot type would be `String`, which owns its allocation and runs `Drop`,
so the family has nowhere `Drop`-free to land.

**Acceptance criteria.**

- `KObject::KString` carries a `&'a str` allocated in its region's bump; `deep_clone` is
  a pointer copy for the `KString` arm.
- `Tagged.tag` and string dict keys carry the same bump-hosted string representation —
  no value-family slot owns a `String`.
- Region death frees string bytes as bump chunks: no per-string `Drop` runs, and no
  typed family cell holds string storage.
- String construction routes a door: the shallow-scalar gate
  ([`is_shallow_scalar`](../../src/machine/model/values/kobject.rs)) no longer
  classes strings as region-free.
- The Miri audit slate is green with region-resident strings exercised.

**Directions.**

- *Tags and keys: arena residence versus interning — open.* A `Tagged` discriminant
  repeats across values of one union; an interned symbol would also make tag
  comparison a handle compare. Either way the stored form is `Copy`, so it clears the
  bump door's bound; the choice fixes the element type every other bump-hosted string
  collection uses, including an operator group's member slice
  ([region-hosted operator groups](region-hosted-operator-groups.md)).
- *Where an interning table would live — open, only if interning wins.* A per-region
  table dies with its region and re-interns a repeated tag per call; a run-global one
  outlives every region and is a `needs_no_pin` eternal member, at the cost of holding
  every symbol for the run.

## Dependencies

**Requires:**

- [Region bump storage for embedder value families](../../workgraph/roadmap/region-bump-storage.md) —
  the public bump door string bytes land in.

**Unblocks:**

- [Region-store expression parts](region-store-expressions.md)
