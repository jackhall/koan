# Callables typed by function types

Every function value typed by its function type, with the expression shape its
bucket registers carried beside it.

**Problem.** [`callable_type`](../../src/elaborate/signature.rs) types a bare
`EXPR` by its expression shape and an `OP` or `UNARY OP` by its operator shape,
and hands back an empty quantifier map for each
([a callable's type](../../src/elaborate/README.md#a-callables-type)). A
combined expression shape is typed by its function type instead, so two
registrations under one bucket answer types of different kinds. The body
runner's frame ([program/body.rs](../../src/program/body.rs)) solves a callee's
`FOR ALL` group only when the callee's type is a function type, so a called bare
`EXPR FOR ALL` binds each of its type parameters to `Any`, and nothing hands the
frame a solution.

**Acceptance criteria.**

- Every function value — a `FN`, a combined expression shape, a bare `EXPR`, an
  `OP` and a `UNARY OP` — is typed by its function type and carries a quantifier
  map: a bare `EXPR` over its head's slot names, a binary `OP` over `left` and
  `right`, a `UNARY OP` over `operands`.
- A function born for a registration carries that registration's expression
  shape, built from its function type over the head once when the function is
  born.
- A called bare `EXPR FOR ALL` binds each type parameter to its group's
  solution against the arguments, which a test reads in the body.

**Directions.**

- *Values carry function types, buckets carry shapes — decided.* The frame then
  has one by-name group solve for every callee, and a selection reads each
  candidate's shape off its function without building one per call.
- *Where the shape rides — open.* On the function node, or beside its quantifier
  map if a second type handle breaks the node's 64-byte size assertion.

## Dependencies

**Requires:** none — foundation.

**Unblocks:**

- [Dispatch](dispatch.md) — selection reads each candidate's shape, and a frame solves every quantified callee.
