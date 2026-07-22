# Region-store expression parts

Terms of art are defined in
[design/value-substrates.md § Vocabulary](../../design/value-substrates.md#vocabulary).

**Problem.** `KExpression` owns its part storage —
`parts: Vec<Spanned<ExpressionPart>>` and the literal-shape vectors inside
[`ExpressionPart`](../../src/machine/model/ast.rs) — making it the largest owner in
the value family: cloning an expression value copies vectors, and every expression
slot carries `Drop` glue.

**Acceptance criteria.**

- `KExpression`'s owned part vectors are arena slices; a `KObject::KExpression`
  value carries no heap-owned `Vec` or `String`.
- Cloning an expression value copies pointers, not part storage.
- Region death for expression storage runs no per-part `Drop`.
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
