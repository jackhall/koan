# Field projection off a record-repr newtype type

`Point.x` with a *type* lhs yields the field's declared type as a type value —
the product-side dual of union member projection.

**Problem.** ATTR's type-lhs member projection
([`attr.rs`](../../src/builtins/attr.rs)`::access_type_member`) answers
signature members, but a record-repr newtype handle falls to the no-members
arm: `Point.x` off the `Point` *type* reports "a type with members" mismatch,
although the field's type sits in the member's sealed `NodeSchema::NewType`
record schema. A writer who wants a field's type — to annotate a slot, to
build a derived type expression — has to restate the field's type by hand,
which drifts when the declaration changes.

**Acceptance criteria.**

- `Point.x` — ATTR with a record-repr-newtype type lhs — yields the field's
  declared type as a type value, including through a `LET`-bound alias of the
  nominal.
- `:(Point.x)` names that type in annotation position; a slot so typed admits
  exactly what a slot spelled with the field's declared type admits.
- An unknown field name errors listing the record's fields; a scalar-repr
  newtype lhs still reports that it has no members.

**Directions.**

- *One projection door — decided.* The read extends `access_type_member`'s
  node-kind ladder; no parallel builtin or new surface syntax.
- *Nested projection — open.* Whether `Point.inner.x` chains through a field
  whose type is itself record-shaped, or stops at one level. Recommended: falls
  out of ordinary ATTR chaining (each step is a type-lhs read), so no special
  casing either way.

## Dependencies

**Requires:** none — a self-contained extension of ATTR's type-lhs ladder.

**Unblocks:** none — a leaf convenience.
