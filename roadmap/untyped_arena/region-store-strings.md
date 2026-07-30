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
  value-family slot holds string storage. A substrate's *index* metadata (a record's
  field-name table, a dict's key map) is not in that residue —
  [drop-free region death](drop-free-region-death.md) converts those.
- String construction routes a door: the shallow-scalar gate
  ([`is_shallow_scalar`](../../src/machine/model/values/kobject.rs)) no longer
  classes strings as region-free, and no bare string crosses a runtime-audited
  move-in — [`resident_in_visiting`](../../src/machine/model/values/kobject.rs)
  answers `false` for a string, so a copying adoption rebuilds it through a
  destination door rather than sharing a pointer its reach has released.
- A region's allocated total — the copy-versus-pin ratio's denominator
  ([`KoanRegionExt::allocated_total`](../../src/machine/core/arena.rs)) — counts its
  bump occupancy ([`Region::bump_bytes`](../../workgraph/src/witnessed/region.rs)),
  so the string bytes a pin would retain are priced.
- The Miri audit slate is green with region-resident strings exercised.

**Directions.**

- *Tags and keys: arena residence versus interning — decided.* Plain bump-hosted
  `&'a str`, no interning table. A run-global table is run-rooted state koan does not
  keep; a per-region one is itself `Drop`-bearing state inside a region being driven
  toward `Drop`-free, and re-interns a repeated tag per call. Tag comparison stays a
  `str` compare. Either form is `Copy`, so it clears the bump door's bound and no
  downstream collection is pinned to the choice — an operator group's member slice
  ([region-hosted operator groups](region-hosted-operator-groups.md)) takes
  `&'a str` entries, and interning remains available later as a pure optimization.
- *Where a string cell's bytes live — decided.* The bump keeps no address table, so
  nothing can ask which region a `&str` points into. A path claiming a region's
  **release** re-bumps the bytes at its destination — every substrate door for a
  top-node string cell, a tag and a string dict key, and the seam's copy verb — so
  such a cell is home-resident and its run verdict stays owned / empty-reach, needing
  no new reach vocabulary. A path that **pins** shares the `&str` verbatim under the
  witness the enclosing fold already composed, which is what keeps `deep_clone` a
  pointer copy.

## Dependencies

**Requires:**

- [The region bump door](../../workgraph/src/witnessed/bump.rs) — shipped substrate:
  `FoldedPlacement::fold_and_bump` is the public door string bytes land in.

**Unblocks:**

- [Region-store expression parts](region-store-expressions.md)
