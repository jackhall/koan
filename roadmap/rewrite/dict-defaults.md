# Dict defaults

A dict literal's `_` key naming the value a lookup of any other key yields.

**Problem.** A dict holds only the keys written in it
([dict key order](../../src/values/README.md#dict-key-order)), so a lookup of
any other key has no answer. A dict literal may write `_` as a key, but only
an arm set reads it: the shape builder refuses a value dict holding one
(`ShapeError::DictDefault`, [scope/shape.rs](../../src/scope/shape.rs)), since
nothing gives its default a lookup, a type, an equality, a rendering or a
lowering.

**Acceptance criteria.**

- A dict literal may write `_` as a key, naming the dict's default: a lookup of
  a key the dict does not hold yields the default, and `{_: x}` alone is a
  constant map.
- A default does not change a dict's type: `{1: "a", _: "b"}` is a
  `MAP Number -> Str`, and a default's type joins into the value type as an
  entry's does.
- Two dicts are equal only when both hold equal defaults or neither holds one.
- A crossing copies a dict's default with it, and a dict weighs its default.

**Directions.**

- *A default is not a key — decided.* It rides beside the dict's sorted key
  and cell runs rather than among them, so no key order places it and a key
  lookup never finds it.
- *A default does not change a dict's type — decided.* A dict with a default
  is typed as one without it; totality is not a type.
- *Where `_` renders — open.* Last, after the key-ordered entries, or first.
  Recommended: last.

## Dependencies

[Code as values](code-values.md) admits `_` as a key of every dict literal, and
refuses a value dict holding one where its shape is built, until this item.

**Requires:**

- [Code as values](code-values.md) — `_` parses as a dict key.
- [Dispatch](dispatch.md) — a dict lookup, which is what observes a default.

**Unblocks:** none — a leaf.
