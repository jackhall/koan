# Catching errors

**Problem.** `TRY` and `CATCH` each have a
[builtin expression shape](../../src/parse/builtin_shapes.rs), and the
[scope builder](../../src/scope/README.md#three-tiers) already builds `TRY`'s
arm shapes, but the evaluator has no reading for either and refuses each with
`nothing evaluates …`. A `TRY` or
`CATCH` operand is walked as an eager part of the enclosing shape, not as a
block of its own. An error carries only `{message :Str}`, so an arm has nothing
to select by. `CATCH`'s declared return is `Any`, not the builtin `Result`
the [builtin table](../../src/dispatch/README.md#the-builtin-table) binds.
[Tutorial 09](../../tutorial/09-errors.md)'s table of error kinds names
refusals the rewrite makes at load (`UnboundName`, `ShapeError`, `ParseError`),
its examples catch an unbound name, and it gives an arm's `frames` as a list of
strings.

**Acceptance criteria.**

- `TRY` runs its body and, on a fault, selects an arm by the fault's kind the
  way `MATCH … UNDER` selects by a variant; a body that yields no fault yields
  its value.
- `TRY` and `CATCH` operands are block shapes, as `MATCH` arms are, so a binder
  inside one binds in that block and is gone after it.
- The scope builder records on a `TRY` body's block that its fault goes to the
  `TRY`'s arms rather than to the enclosing frame, and a `TRY` body is never in
  tail position.
- The builtin `Error` is a union with one unparameterized variant per fault
  kind, each over a record of that kind's fields, values among them, and
  `frames`, the trace an uncaught fault prints, as a list of records.
- `CATCH` turns its body's outcome into a `Result` value: `Result.Ok` of the
  value, or `Result.Error` of the caught `Error`.
- `CATCH`'s declared return names the builtin `Result`.
- A `TRY` or `CATCH` releases the frames a fault kept once it has built the
  caught `Error`.
- Tutorial 09 teaches the fault kinds a running program meets, and every
  tutorial snippet using `TRY`, `CATCH` or `Result` runs on the rewritten
  stack; `tools/verify_snippets.py`'s pending list no longer names them.

**Directions.**

- *What an error carries — decided.* `Error` is a builtin union with one
  variant per fault kind, so `TRY` selects as `MATCH … UNDER Error` does, and a
  misspelled kind is refused where the shape is built. Its variants are not
  parameterized: a field whose type varies with the refusal, such as a missing
  key, is `Any`. A field may hold a value, since the fault keeps the frame it
  lies in until the catcher copies it out.
- *`TRY`'s declared result — open.* `TRY … -> <type> WITH` declares its result
  type. A `MATCH` declares none, its type computed from its arms
  ([matching](../../src/dispatch/README.md#matching)), since a declared result that retypes makes a tail
  arm's outcome differ from a non-tail one's. `TRY` may drop its declared
  result likewise, or keep it, since its body is never in tail position.

## Dependencies

**Requires:**

- [Faults and call traces](faults.md) — what `TRY` and `CATCH` catch, and the frames they read.
- [The builtin table](../../src/dispatch/README.md#the-builtin-table) — shipped:
  the bound `Result` `CATCH` returns.

**Unblocks:** none.
