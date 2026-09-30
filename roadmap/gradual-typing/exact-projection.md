# An exact projection

`FROM` typed exactly where the load knows its record's type.

**Problem.** `FROM`'s native builds its record from the field values
(`Native::Project` in [`builtins`](../../src/dispatch/builtins.rs)), and a
native is never retyped, so the result carries the fields' own types rather
than the record's: the
[retype is shallow](../../src/values/README.md#the-type-memo-and-satisfies), so
after `LET r = ({x = 1, y = "a"} :! {x :(Number | Str), y :Str})`,
`#[x] FROM r` carries `{x :Number}`. The load therefore types a call of `FROM`
at most the projection of its record's static upper end, never exactly
([static types](../../src/dispatch/README.md#static-types)), and the builtin
table's test exempts it from "no builtin declares a return the retype makes
exact". A written list naming a field twice, `#[x y x] FROM r`, hands
`Record::new` a repeated name and trips its debug assertion that a record's
field names are distinct.

**Acceptance criteria.**

- `#[x] FROM r` above carries `{x :(Number | Str)}`: the result is retyped to
  the projection of the record's carried type.
- Where the record's static type is exact and the field list is written,
  the load types the call exactly that projection, and a use over it whose
  candidates the exact type decides is selected at load.
- The builtin table's test exempts no builtin.
- A field list naming a field twice never builds a record with a repeated
  name.

**Directions.**

- *A repeated name — open.* The call names each field once, as the load's
  projection already does, or it refuses the list: a fault, and a load refusal
  where the list is written. Recommended: once.

## Dependencies

**Requires:** none.

**Unblocks:** none — a leaf.
