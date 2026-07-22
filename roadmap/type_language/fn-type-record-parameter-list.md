# Record-typed parameter list in the FN type constructor

The `FN` type constructor's parameter list is written as a record type
expression — `:(FN :{x :Number} -> Number)` — not a parenthesized field list.

**Problem.** The `FN` *type constructor* — the `:(FN … -> …)` form registered
in [`parameterized_types.rs`](../../src/builtins/parameterized_types.rs) beside
`LIST OF` / `MAP` / `AS` — takes its parameter list as a parenthesized field
list: `:(FN (x :Number) -> Number)`. The slot is typed `KEXPRESSION`, captured
raw, and walked by the shared typed-field-list parser. The record type
expression `:(FN :{x :Number} -> Number)` does not dispatch: a `:{…}` part is
`RecordType`, which the `KEXPRESSION` slot rejects, so it falls through the
bucket with `no matching function`. This is the last parenthesized-field-list
surface in the type language — the anonymous definition form takes its schema
as `FN :{s :Str} -> … = …`, and `:{…}` is the record-type syntax everywhere
else. Rendering mirrors the parenthesized form: a function type prints as
`:(FN (x :Number) -> Number)`.

**Acceptance criteria.**

- `:(FN :{x :Number} -> Number)` elaborates to the function type, and
  `:(FN :{} -> Str)` to its zero-parameter counterpart, wherever a type
  expression is accepted.
- A function type value renders with the record form.
- Tutorial and reference examples of function types use the record form.

**Directions.**

- *Slot carrier — open.* Either a `RECORD_TYPE` slot capturing the `:{…}` part
  raw and feeding the same field-list elaboration the current `KEXPRESSION`
  slot uses, or an `of_kind(ProperType)` slot that lets the record type
  sub-dispatch to a resolved record `KType` first, mirroring the definition
  form's record-schema overload in
  [`fn_def.rs`](../../src/builtins/fn_def.rs).
- *Fate of the parenthesized form — open.* Reject it with a diagnostic
  pointing at the record spelling, or admit it transitionally while the doc
  tree migrates.

## Dependencies

**Requires:** none — a self-contained constructor change.

**Unblocks:**

- [Bare parenthesized return annotations](function-typed-return-annotations.md)
  — its closure-factory criteria write function types in the record form.
