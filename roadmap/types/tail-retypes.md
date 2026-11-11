# A tail call keeps every retype

A declared return that yields the same value wherever its call is placed.

**Problem.** A tail hop skips the inner retype. A hop's
[`Contract`](../../src/program/record.rs) keeps one type to check, the
innermost callee's return, and one to retype to, the outermost's
([frames, contracts and tails](../../src/program/README.md#frames-contracts-and-tails);
`frame` in [body.rs](../../src/program/body.rs)). Where the outermost return
re-stamps less than the inner one, a tail call yields a different value than
the same call bound first:

```koan
EXPR #(INNER) -> :{x :Number} = #({x = 1, y = 2})
EXPR #(OUTER) -> Any = #(INNER)
EXPR #(KEEPS) -> Any = #(
  LET v = (INNER)
  v
)
PRINT (OUTER)
PRINT (KEEPS)
```

prints `{x = 1, y = 2}`, then `{x = 1}`.

**Acceptance criteria.**

- A call in tail position yields the same value as the same call bound to a
  name and returned: `OUTER` and `KEEPS` above print the same record value.
- A tail recursion N deep still holds a constant number of cells.

**Directions.**

- *How a hop keeps every retype — open.* The contract carries each pending
  retype in a chain bounded by the source's nesting, applied in order where
  the value finishes; or a call tails only where the outermost retype
  re-stamps everything the inner one would, and runs as a child otherwise.

## Dependencies

**Requires:** none.

**Unblocks:** none — a leaf.
