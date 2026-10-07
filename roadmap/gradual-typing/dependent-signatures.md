# Dependent signatures

A callable's signature reading its own parameters.

**Problem.** A callable's parameter and return types are elaborated before any
argument exists, and read no parameter: `EXPR #(ECHO elem :Ordered) ->
elem.Carrier = …` loads with "`elem` names no binding visible here", so no
signature says it returns a value of its argument module's own type. Nothing
names the type a value carries in type position: `TYPE OF` has no shape.

**Acceptance criteria.**

- A slot's or a return's type reads a type member of an earlier parameter:
  `-> elem.Carrier` is, at each call, the `Carrier` of the module passed as
  `elem`, and the load reads it through the argument's path.
- `:(TYPE OF <value>)` names the type a value carries: `-> :(TYPE OF elem)`
  returns a value of the argument's own type, resolved per call, and
  `m :(TYPE OF int_order)` admits a module that fits `int_order`'s signature.

**Directions.**

- *Which parameters a type may read — open.* Only earlier slots, or any slot of
  the record, which orders a call's admission by what each slot's type reads.

## Dependencies

**Requires:**

- [Path types](../rewrite/path-types.md) — a parameter's member is a path
  through a leaf.

**Unblocks:** none — a leaf.
