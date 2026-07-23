# Region-store string values

Terms of art are defined in
[design/value-substrates.md § Vocabulary](../../design/value-substrates.md#vocabulary).

**Problem.** String bytes are owned wherever they appear in the value family:
`KObject::KString(String)`, the `Tagged { tag: String, .. }` discriminant, and
`KKey::String(String)` dict keys ([kkey.rs](../../src/machine/model/values/kkey.rs))
each own a heap allocation, so `deep_clone` copies bytes for the `KString` arm and
every value-family slot that can hold a string carries `Drop` glue — blocking the
untyped-arena move for the whole family.

**Acceptance criteria.**

- `KObject::KString` carries an arena `&'a str`; `deep_clone` is a pointer copy for
  the `KString` arm.
- `Tagged.tag` and string dict keys carry arena-resident (or interned) string data —
  no value-family slot owns a `String`.
- String construction routes a door: the shallow-scalar gate
  ([`is_shallow_scalar`](../../src/machine/model/values/kobject.rs)) no longer
  classes strings as region-free.
- The Miri audit slate is green with region-resident strings exercised.

**Directions.**

- *Tags and keys: arena residence versus interning — open.* A `Tagged` discriminant
  repeats across values of one union; an interned symbol would also make tag
  comparison a handle compare.

## Dependencies

**Requires:**


**Unblocks:**

- [Region-store expression parts](region-store-expressions.md)
