# Binders nested in expressions

A name binder written inside an expression, rewritten where the body shape is
built into a statement of its own.

**Problem.** The [shape builder](../../src/scope/README.md#three-tiers) records a
binder only at a statement's root — `statement_binder` in
[scope/shape/build.rs](../../src/scope/shape/build.rs) holds one slot per
statement — so a `LET` inside an expression, `PRINT (LET doubled = 42)`,
declares nothing: a later read of its name is unbound, and nothing binds it. An
evaluation cannot bind a slot in its place, since it holds only an activation's
read view and binding is the [body runner](../../src/program/README.md#the-body-runner)'s.
`TRY` and `CATCH` operands are walked as eager parts of the enclosing shape, not
as blocks of their own.

**Acceptance criteria.**

- A binder that binds a name — `LET`, `NEWTYPE`, `UNION`, `MODULE` and every
  other name binder — may sit anywhere among an expression's eager parts. The
  shape builder hoists it, and every part before it that is not a name, a type,
  a literal or a quote, into statements of the enclosing body in source order,
  so effects keep source order and no reader past the builder meets a nested
  binder.
- A nested binder is visible to the rest of its own statement, left to right,
  and to every statement after it.
- The expression a nested binder sat in reads the bound name in its place:
  `PRINT (LET doubled = 42)` becomes `LET doubled = 42` followed by
  `PRINT doubled`.
- A binder that binds an expression shape — a bare `EXPR` or `OP`, or a
  combined `LET … = FN EXPR …` or `OP` — nested in an expression is refused
  where the shape is built.
- The hoist never crosses a block shape, a callable body or a quote. `TRY` and
  `CATCH` operands are block shapes, as `MATCH` arms are, so a binder inside one
  binds in that block and is gone after it.

**Directions.**

- *A rewrite, not a binding by the evaluator — decided.* Binding stays the body
  runner's; the hoist is built through `parse`'s node constructor, as the
  [pairwise rewrite](../../src/scope/README.md#operator-groups) is, and reuses
  its anonymous-slot naming.

## Dependencies

**Requires:** none — foundation.

**Unblocks:**

- [Dispatch](dispatch.md) — a statement holding a nested binder is an ordinary statement once hoisted.
- [Control expression shapes and errors](control-and-errors.md) — `TRY` and `CATCH` operands run as blocks.
