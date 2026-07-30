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

- `ExpressionPart` and `Spanned` are `Copy`: every owned vector and `String` inside a
  part is a slice or string reference at the part's lifetime.
- `KExpression`'s parts are one slice type — `&'a [Spanned<ExpressionPart<'a>>]` — that
  both program-lifetime storage and a region's bump hand back, so copying an expression
  to a node for splicing is a slice copy rather than a rebuild.
- A `KObject::KExpression` value carries no heap-owned `Vec` or `String`, and cloning one
  copies pointers, not part storage.
- Region death for a spliced expression's part storage runs no per-part `Drop`.
- An expression whose parts live only in program-lifetime storage has empty reach — the
  storage is a `needs_no_pin` eternal member, so pointing at program text pins nothing.
- A splice-carrying expression cell entering a sectioned container carries an
  exact run description, composed by the door from the carriers of the splice operands
  the part slice was built from; the conservative operand-envelope verdict at the
  sectioned alloc door is deleted.
- The Miri audit slate is green with region-resident expressions exercised.

**Directions.**

- *Where parse-time ASTs live — decided.* Program text and the raw AST live in a
  program-lifetime bump outside the region model, which may outlive even the run-root
  region; only the per-node copies made for splicing are region-bump allocated. Both
  storages produce the same part-slice type, which is what makes the split invisible to
  `KExpression` and the copy a slice copy.

## Dependencies

**Requires:**

- [Region bump storage for embedder value families](../../workgraph/roadmap/region-bump-storage.md) —
  the public bump door spliced part slices land in.
- [Region-store string values](region-store-strings.md) — expression parts embed
  strings; project conversion order makes expressions the last substrate to move.

**Unblocks:**

- [Drop-free region death](drop-free-region-death.md)
