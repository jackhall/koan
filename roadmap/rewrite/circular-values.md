# Circular values

Data values that refer to themselves, born in a knot beside the functions that
capture them.

**Problem.** [`function`](../../src/function/README.md#the-tie) ties a deferred-only
component as a [knot](../../src/memory/README.md#the-knot) only when every
member is a callable binder. A component with a data member — `LET a = [f];
LET f = FN <reads a>`, or a ring of containers — is refused, although the
scopes' visibility rule admits it
([src/scope/README.md](../../src/scope/README.md#visibility)): a container
cell can hold nothing but a value word, and a value word has no arm that names
a fellow knot node, so a container cannot be a node. `values` has no equality
that terminates on a cycle, no renderer that does, and no copy that rebases a
member reference held inside a container's cell run.

**Acceptance criteria.**

- A deferred-only component with data members is born as one knot, and a
  container that is a member holds its sibling references so that a copy of
  the knot rebases them.
- A deferred mention below a nested constructor (`LET a = {inner: [b]}` with
  `b` in `a`'s component) is either written into the knot or rejected where
  the shape is built, and the choice is documented.
- `==` over two circular values is a bisimulation over `(knot, index)` pairs,
  and `PRINT` of one terminates.
- A copied circular value is the same graph at the destination.

**Directions.**

- *How a container cell names a node — open.* A `Value` arm holding a knot
  member (pointer-carrying, so a copy must rebase it against the knot being
  copied), or a container variant whose cells are edges rather than values.
- *Nested deferred mentions — open.* Either the tie writes each nested
  constructor on the path as an anonymous node of the knot, which plan-then-tie
  permits, or the shape classifies such a mention as eager and the program is
  rejected.

## Dependencies

**Requires:** none — [functions](../../src/function/README.md) ship the tie this item extends.

**Unblocks:** none — a leaf of the rewrite.
