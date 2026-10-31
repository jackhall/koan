# Composing inside literals

A `$..` splice in a group a literal or a template brace holds, composed as it
is in a paren group.

**Problem.** Composing a quote at its birth descends a node's own groups and
stops at a container literal ([`Composer::node`](../../src/knot/view.rs)), so a
splice in a group a list, dict or record literal's element holds, or in a
[template](../../src/parse/README.md#templates)'s brace, would never be
spliced. The parse refuses each such group instead (`SPLICE_UNDER_LITERAL` in
[lower.rs](../../src/parse/lower.rs)): `#(PRINT [($..xs)])`,
`#({a = (f $..xs)})` and `#(%"sum {$..parts}")` do not load. Splicing's rule
([splicing](../../src/scope/README.md#splicing)) is that a spliced part
composes as any part does, and a brace holds one expression as a paren group
does, so these are the rule's one gap.

**Acceptance criteria.**

- With `LET parts = [#(1 + 2)]`, `EVAL #(PRINT [($..parts)]) -> Any` prints
  `[3]`, and `EVAL #(%"sum {$..parts}") -> Str` answers `sum 3`.
- A splice in a group a dict's key or value or a record's field value holds
  composes as in a paren group: `EVAL #({a = ($..parts)}) -> Any` is `{a = 3}`.
- A splice under a literal keeps the rules a splice in a paren group has: its
  level is read from the parts around it, the depth limit holds, and the load
  and the birth refuse what they refuse in a paren group.
- The parse no longer refuses a splice in a group a literal or a template
  brace holds.

**Directions.**

- *A code view over a literal — open.* The code view has no item for a
  literal one of whose elements composes. Options: a code item wrapping a
  literal, with its elements as children the materializer lays
  ([src/knot/view.rs](../../src/knot/view.rs): `CodeItem`, `Wrap`, the
  materializer's `Laid`, the summary, the digest and equality); or composing
  each such element as a code view of its own and laying the literal flat
  around them at birth.

## Dependencies

**Requires:** none — the code view and templates it extends are shipped.

**Unblocks:** none — a leaf.
