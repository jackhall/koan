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

- *How a schema is written — open.* `#{x: null, y: #[z]}` quotes each value, so
  `#[z]` arrives as code. A schema of symbols waits on how the
  [metaprogramming](../metaprogramming/README.md) project represents names as
  data.

## Dependencies

**Requires:**

- [A value's type is its surface](type-is-the-surface.md) — the shallow projection it nests.
- [Code names](../metaprogramming/code-names.md) — the end of the metaprogramming project.

**Unblocks:** none — a leaf.
