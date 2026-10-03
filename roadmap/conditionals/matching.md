# Matching

Branching on a value's type, on a union's variant, and on a value.

**Problem.** `MATCH` and `MATCH … OVER` each have a
[builtin expression shape](../../src/parse/builtin_shapes.rs) and arm shapes the
[scope builder](../../src/scope/README.md#three-tiers) already builds, but the
evaluator answers each with an error saying it does not run yet. A
`MATCH … WITH` guard is a type, and a value guard (`true`, `1`) is refused
`Inadmissible` where the shape is built. The scope builder records whether an
arm's last statement is in tail position, but
[a block never tails](../../src/program/README.md#frames-contracts-and-tails). A
one-variant union collapses to its variant, so `One.Only` is refused as naming
no member of `Only`. The load's [static types](../../src/dispatch/README.md#static-types)
read an arm's `it` as `Any`.

**Acceptance criteria.**

- `MATCH` over a union is spelled `MATCH <scrutinee> UNDER <union> -> <type>
  WITH <arms>`, in the builtin table and in the tutorial.
- `MATCH` and `MATCH … UNDER` select the unique most specific arm whose guard
  admits the scrutinee, as dispatch selects a candidate, bind `it` in that
  arm's block and yield its value, checked against the declared result type.
  No admitting arm is an error, and so are two equally specific admitting
  arms.
- A guard written twice in one arm set is refused where the shape is built.
- Under `MATCH … UNDER`, a label naming no variant of the union is an error,
  and so is a variant with no arm where no `_` arm is written; both are found
  when the `MATCH` runs, before an arm is selected.
- A one-variant union reads like any other: `One.Only` names its variant, and
  `MATCH … UNDER One` selects by it.
- An arm whose `MATCH` is in tail position tails, so a recursion through `MATCH`
  arms N deep holds a constant number of cells.
- `MATCH` branches on a scrutinee's value as well as its type.
- An arm's `it` has the arm's guard as its static type.
- The tutorial snippets using `MATCH` without `TRY`, `CATCH` or `Result` run on
  the rewritten stack, and `tools/verify_snippets.py`'s pending list no longer
  names `MATCH`.

**Directions.**

- *`MATCH … UNDER` — decided.* The union clause claims the scrutinee's type
  lies under the union, the relation
  [a bound's `UNDER`](../../tutorial/12-functors.md#bounding-a-type-parameter)
  names. `OVER` is left naming
  a domain: an operator's operand type, and the captures `CLOSE OVER` copies.
- *An arm is a block — decided.* An arm runs as the block shape the scope
  builder already builds for it, with `it` its one parameter, through the same
  block evaluation dispatch uses for a synthesized block.
- *Arms select by specificity — decided.* Arms are written as a dict of
  quotes, guards to blocks — typed `Dict(TypeCode, Block)` under `MATCH`'s type
  guards and `Dict(Name, Block)` under `MATCH … UNDER`'s and `TRY`'s labels
  ([the builtin shape table](../../src/parse/README.md#the-builtin-shape-table-one-typed-entry-every-fact))
  — so their written order says nothing; the most specific admitting guard is
  chosen, by the order dispatch ranks candidates by.
- *Arms against their union — decided.* The union is a type value elaborated
  where the `MATCH` runs, so its labels and coverage are checked there rather
  where the shape is built.
- *Guards and results are handles — decided.* A type guard, the `MATCH`'s
  written result and the enclosing callable's declared return elaborate where
  the shape is built
  ([the type channel at load](../../src/elaborate/README.md#the-type-channel-at-load)), so a guard
  written twice is one handle written twice however each is spelled, and
  whether an arm's `MATCH` in tail position tails is a fact the shape records,
  not one the contract decides when the `MATCH` runs.
- *`MATCH` on values — open.* How a value guard is written, typed and ranked
  against a type guard is undecided.
- *Arms narrowed where the shape is built — open.* An arm set whose scrutinee
  has a static type could drop, and refuse, its guards as
  [static selection](../../src/dispatch/README.md#static-types) drops a
  keyworded use's candidates.

## Dependencies

**Requires:** none.

**Unblocks:**

- [Catching errors](catching.md) — `TRY` selects an arm as `MATCH … UNDER` does.
- [Retire the old runtime](../rewrite/retire-the-old-runtime.md) — the branching surface `machine` still owns.
