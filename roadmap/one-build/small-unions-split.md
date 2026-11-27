# A small union split where it is bound

**Problem.** A name declared at a union is typed at most that union, so a
use of it that the members would answer differently is left to the call:

```text
EXPR #(SHOW n :Number) -> Str = #("number")
EXPR #(SHOW s :Str) -> Str = #("string")
EXPR #(DESCRIBE v :(Number | Str)) -> Str = #(SHOW v)
```

`SHOW v` is *maybe* for both candidates, and every call of `DESCRIBE` admits
and ranks them again from the type `v` carries
([where a call runs](../../design/gradual-typing.md#where-a-call-runs)),
though the text names both members and what each selects.

**Acceptance criteria.**

- A name declared at a union asks which member it carries, and its bind site
  has one `BoundInstance` per member
  ([what a bind site binds](../../design/binding-graph.md#what-a-bind-site-binds));
  `DESCRIBE` above has two, each with its `SHOW` selected at build.
- A frame picks its instance by one test of the carried type as it binds; a
  test counts admissions of `SHOW` across calls of `DESCRIBE` and counts
  none.
- A body is split while the combinations of its split names, across all of
  its bind sites, number at most twelve; a body past twelve is not split, and its members are told apart as
  the program runs, as before.
- Every program's output equals its output unsplit.

**Directions.**

- *Which names split — open.* A parameter and a statement's binder, each a
  single value with one answer per frame. A union inside a list or a dict has
  an answer per element, so it splits where an element is bound to a name — a
  `MATCH` arm's `it`, a `LET` — and never at the container's bind site. A
  record value's field is a single value and could split at the record's
  bind site, or wait until the field is bound to a name. Recommended: names
  only, a field when it is bound.
- *What counts toward twelve — decided.* The product over every bind site
  of the body, a statement's split multiplying the entry's.
- *Past twelve — decided.* The body is not split and is not refused: its
  instances are bounded either way, and the members are a declared type.

## Dependencies

**Requires:** none.

**Unblocks:** none — a leaf.
