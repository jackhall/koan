# A nested projection

`FROM` projecting a record's subrecords by a schema.

**Problem.** `FROM`'s field list names a record's top-level fields alone
(`Native::Project` in [`builtins`](../../src/dispatch/builtins.rs)), so
narrowing a subrecord takes a `FROM` per level and a record literal rebuilding
the result: projecting `y` of `r = {x = 1, y = {z = 2, w = 3}}` onto `z` is
`{x = r.x, y = (#[z] FROM r.y)}`.

**Acceptance criteria.**

- A schema maps each field name to `null`, which keeps the field whole, or to a
  schema, which projects the subrecord under it. `FROM` over a schema is its
  record retyped to the projection of its carried type the schema describes,
  and a list of names is the schema mapping each to `null`.
- The load types the call from both ends of the record's static type, nested
  fields included.
- A schema is a data structure of symbols, and `FROM` parses no code to read
  one.

**Directions.**

- *How a schema is written — open.* `#[z]` is the list `[#(z)]` of `Name`
  quotes ([quoting each element](../../tutorial/10-quoting.md#quoting-each-element-of-a-list-or-dict)),
  so `#{x: null, y: #[z]}` is a dict of `null` and lists of names, and reading
  a `Name` quote as a field is a read, not a parse. Recommended: that
  spelling, with `null` the only non-code value a schema holds.

## Dependencies

**Requires:** none — `FROM` and name quotes have shipped.

**Unblocks:** none — a leaf.
