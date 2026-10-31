# Matching

Branching on a value's type, on a union's variant, and on a value.

**Problem.** `MATCH` and `MATCH … OVER` each have a
[builtin expression shape](../../src/parse/builtin_shapes.rs) and arm shapes the
[scope builder](../../src/scope/README.md#three-tiers) already builds, but the
evaluator has no reading for either and refuses each with `nothing evaluates …`.
Each declares its result type, `-> <type>`. A `MATCH … WITH` guard is a type,
and a value guard (`true`, `1`) is refused `Inadmissible` where the shape is
built. A type guard written twice in one arm set is refused `RepeatedGuard`
there, but a label written twice under `MATCH … OVER` is not. The scope builder
records whether an arm's last statement is in tail position, but
[a block never tails](../../src/program/README.md#frames-contracts-and-tails). A
one-variant union collapses to its variant, so `One.Only` is refused as naming
no member of `Only`. The load's [static types](../../src/dispatch/README.md#static-types)
read an arm's `it` as at most `Any`.

**Acceptance criteria.**

- `MATCH` is spelled `MATCH <scrutinee> WITH <arms>`, and over a union
  `MATCH <scrutinee> UNDER <union> WITH <arms>`, declaring no result type, in
  the builtin table and in the tutorial.
- `MATCH` and `MATCH … UNDER` select the unique most specific arm whose guard
  admits the scrutinee, as dispatch selects a candidate, bind `it` in that
  arm's block and yield its value. No admitting arm is an error, and so are two
  equally specific admitting arms.
- A `MATCH`'s static type spans the static types of the arms the load leaves
  selectable: decided in the load where each is closed, and in each settlement
  where one is over a `FOR ALL` name.
- A guard written twice in one arm set is refused where the shape is built.
- Under `MATCH … UNDER`, a label naming no variant of the union is an error,
  and so is a variant with no arm where no `_` arm is written, and a union
  clause that is no union; each is found where the union's binding is decided
  ([where facts are decided](../../src/build/README.md#where-facts-are-decided)),
  once under each binding and before any arm is selected under it: in the
  load for a closed union, never when the `MATCH` runs.
- A `UNION` declaring one variant is refused where it is declared, and the
  message points at `NEWTYPE`.
- An arm whose `MATCH` is in tail position tails, so a recursion through `MATCH`
  arms N deep holds a constant number of cells.
- A recursion through `MATCH` arms keeps what each round's tail call hands on
  and little more: a walk down an N-link union chain holds the same memory at
  every N, and a loop building an accumulator holds the accumulator and a
  constant more, as [tutorial 06](../../tutorial/06-pattern-matching.md) says.
- `MATCH … WITH` takes a scalar literal as a guard, which admits a scrutinee
  equal to it and outranks every type guard.
- An arm's `it` is retyped to its guard, as a parameter is to its declared
  type, and has that type as its static type: a type guard's type, a value
  guard's scalar type, and under a label the variant's payload at the variant's
  representation.
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
  quotes, guards to blocks — typed `Dict(:(TypeCode | Literal), Block)` under
  `MATCH`'s guards and `Dict(Name, Block)` under `MATCH … UNDER`'s and `TRY`'s
  labels
  ([the builtin shape table](../../src/parse/README.md#the-builtin-shape-table-one-typed-entry-every-fact))
  — so their written order says nothing; the most specific admitting guard is
  chosen, by the order dispatch ranks candidates by. The ranking is a fact of
  the site, recorded where the shape is built for closed guards as dispatch
  records a use's elimination table, not ranked again at each evaluation.
- *Arms against their union — decided.* The union clause is a type expression
  the unit records, so its labels and coverage are checked beside
  [the guards' check](../../src/elaborate/README.md#the-type-channel-at-load)
  in the walk, and the check lands wherever the union's binding closes: the
  load for a closed union, a settlement for a rigid one, a frame's activation
  for one bound to run content.
- *Guards, the union and the return are handles — decided.* A type guard, the
  union clause and the enclosing callable's declared return elaborate where the
  shape is built
  ([the type channel at load](../../src/elaborate/README.md#the-type-channel-at-load)), so a guard
  written twice is one handle written twice however each is spelled, and
  whether an arm's `MATCH` in tail position tails is a fact the shape records,
  not one the contract decides when the `MATCH` runs.
- *No declared result — decided.* A `MATCH`'s type is computed from its arms,
  and a reader who wants it checked ascribes it with `:!`. A tail arm then owes
  exactly the contract its frame owes. A declared result that retyped the
  value would make a tail arm's outcome differ from a non-tail one's, since a
  hop's contract folds checks exactly only along an ordered chain.
- *`MATCH` on values — decided.* A value guard is a scalar literal — a number,
  a string, `true`, `false` or `null` — written as a key of the `WITH` dict
  beside the type guards. It admits a scrutinee `==` to it, so a tagged value
  never matches a scalar literal, it outranks every type guard, and `it` is
  exactly the literal's scalar type. The arm slot's type is a new lattice
  constant, `Dict(:(TypeCode | Literal), Block)`.
- *Arms narrowed where the shape is built — decided.* Arms are narrowed against
  the scrutinee's static type as
  [static selection](../../src/dispatch/README.md#static-types) narrows a
  keyworded use's candidates. An arm that can never admit is dropped, not
  refused, since coverage forces arms a narrow scrutinee never reaches. An arm
  that may admit is dropped where an arm that always admits outranks it. No arm
  left, or arms that always admit with none ranking first, refuse the load. A
  lone survivor is recorded as the arm the run takes.
- *A one-variant union — decided.* A one-variant union is the same type as its
  variant, since a variant unifies with its standalone twin
  ([identity](../../lattice/src/types/identity.md)), so its declaration is
  refused rather than read as a union. `NEWTYPE` declares a single tagged type.
- *What a `MATCH` loop keeps — decided.* `it` is held beside its scrutinee's
  kept part, a payload one path segment below it, and each tail arm's last
  statement joins its callable's
  [threaded set](../../src/scope/README.md#the-threaded-set), so a loop through
  arms keeps what a loop through a callable's last statement keeps.

## Dependencies

**Requires:** none.

**Unblocks:**

- [Catching errors](catching.md) — `TRY` selects an arm as `MATCH … UNDER` does.
