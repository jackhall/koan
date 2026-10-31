# Return contracts

A declared return that yields the same value wherever its call is placed, and
is refused at load wherever it can never hold.

**Problem.** Two gaps in how a declared return holds:

- **A tail hop skips the inner retype.** A hop's
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
- **A load-time "can never satisfy" check reads one end.** Every such check —
  a return, an ascription, an annotation, an `EVAL`'s code — is
  `never_satisfies`, which refuses only where the static type's upper end
  meets the wanted type at `Never`
  ([static types](../../src/dispatch/README.md#static-types)). A value carries
  a type at or above its static type's lower end, so a lower end that does not
  lie under the wanted type can never satisfy it either, yet the load passes
  it:

  ```koan
  EXPR #(LISTY) -> :(LIST OF Any) = #([1])
  EXPR #(NUMS) -> :(LIST OF Number) = #(LISTY)
  ```

  loads, and every call of `NUMS` faults with `returned :(LIST OF Any), which
  does not satisfy :(LIST OF Number)`.

**Acceptance criteria.**

- A call in tail position yields the same value as the same call bound to a
  name and returned: `OUTER` and `KEEPS` above print the same record.
- A tail recursion N deep still holds a constant number of cells.
- A body, ascription, annotation or `EVAL`'s code whose static type's lower end
  does not lie under the type it must satisfy refuses the load: `NUMS` above
  is refused, its message naming both types.

**Directions.**

- *How a hop keeps every retype — open.* The contract carries each pending
  retype in a chain bounded by the source's nesting, applied in order where
  the value finishes; or a call tails only where the outermost retype
  re-stamps everything the inner one would, and runs as a child otherwise.
- *A lower end over a `FOR ALL` name — open.* The order holds no variables, so
  a lower end over a rigid name is decided where a settlement closes it, or
  left to the run.

## Dependencies

**Requires:** none.

**Unblocks:** none — a leaf.
