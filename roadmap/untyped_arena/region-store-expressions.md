# Region-store expression parts

Terms of art are defined in
[design/value-substrates.md § Vocabulary](../../design/value-substrates.md#vocabulary).

**Problem.** `KExpression` owns its part storage —
`parts: Vec<Spanned<ExpressionPart>>` and the literal-shape vectors inside
[`ExpressionPart`](../../src/machine/model/ast.rs) — making it the largest owner in
the value family: cloning an expression value copies vectors, and every expression
slot carries `Drop` glue. Because a splice-carrying expression has no stored reach
of its own, the sectioned alloc door
([design/value-substrates.md § Sectioned reach](../../design/value-substrates.md#sectioned-reach))
gives an expression cell a conservative envelope-wide run description — sound but
wider than exact, the one cell family run exactness does not yet cover.

**Acceptance criteria.**

- `KExpression`'s owned part vectors are arena slices; a `KObject::KExpression`
  value carries no heap-owned `Vec` or `String`.
- Cloning an expression value copies pointers, not part storage.
- Region death for expression storage runs no per-part `Drop`.
- A splice-carrying expression cell entering a sectioned container carries an
  exact run description composed from its embedded values' stored reach; the
  conservative operand-envelope verdict at the sectioned alloc door is deleted.
- The Miri audit slate is green with region-resident expressions exercised.

**Directions.**

- *Where parse-time ASTs live — open.* The parser builds expressions before any call
  region exists; decide which arena homes program text (the run-root region, or a
  dedicated AST arena with the same borrow discipline).

## Dependencies

**Requires:**

- [Region-store string values](region-store-strings.md) — expression parts embed
  strings; project conversion order makes expressions the last substrate to move.

**Unblocks:**

- [Drop-free region death](drop-free-region-death.md)
