# Control expression shapes and errors

Branching on a value, and catching an error.

**Problem.** [Dispatch](dispatch.md) runs a program's values, names, functions
and keyworded calls, and an error there is a tagged value of the builtin
`Error` over `{message :Str}` that ends the program when nothing catches it —
and nothing can. `MATCH`, `MATCH … OVER`, `TRY` and `CATCH` each have a
[builtin expression shape](../../src/parse/builtin_shapes.rs) and arm shapes the
[scope builder](../../src/scope/README.md#three-tiers) already builds, but the
evaluator answers each with an error saying it does not run yet. The builtin
`Result` union has a declaration
([`builtin_result`](../../src/elaborate/builtin.rs)) but no builtin-table entry,
and `CATCH`'s declared return is `Any`.

**Acceptance criteria.**

- `MATCH`'s union form is spelled `MATCH <scrutinee> UNDER <union> -> <type>
  WITH <arms>`, in the builtin table and in the tutorial.
- `MATCH` and `MATCH … UNDER` select the first arm whose head admits the
  scrutinee, bind `it` in that arm's block and yield its value, checked against
  the declared result type; no admitting arm is an error.
- `TRY` runs its body and, on an error value, selects an arm by the error the
  way `MATCH` selects by a value; a body that yields no error yields its value.
- `CATCH` turns its body's outcome into a `Result` value: `Result.Ok` of the
  value, or `Result.Error` of the error.
- The builtin `Result` union is declared in the builtin table, and `CATCH`'s
  declared return names it.
- The tutorial snippets using `MATCH`, `TRY`, `CATCH` or `Result` run on the
  rewritten stack, and `tools/verify_snippets.py`'s pending list no longer
  names them.

**Directions.**

- *What an error carries — open.* Dispatch's payload is `{message :Str}`. A
  `TRY` arm selecting by kind needs a discriminant: a `kind` field of a builtin
  union, or one `Error` variant per kind. Recommended: decide when the arms'
  heads are written, against the tutorial's `TRY` snippets.
- *`MATCH … UNDER` — decided.* The union clause claims the scrutinee's type
  lies under the union, the relation
  [a bound's `UNDER`](../../tutorial/12-functors.md#bounding-a-type-parameter-under)
  names. `OVER` is left naming
  a domain: an operator's operand type, and the captures `CLOSE OVER` copies.
- *An arm is a block — decided.* An arm runs as the block shape the scope
  builder already builds for it, with `it` its one parameter, through the same
  block evaluation dispatch uses for a synthesized block.

## Dependencies

**Requires:**

- [Dispatch](dispatch.md) — the evaluator, block evaluation and error values.
- [Binders nested in expressions](nested-binders.md) — `TRY` and `CATCH` operands are block shapes.

**Unblocks:**

- [Retire the old runtime](retire-the-old-runtime.md) — the control surface `machine` still owns.
