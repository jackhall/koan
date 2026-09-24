# Lambdas born where they are written

A function value for a `FN` that no binder names.

**Problem.** The [tie](../../src/knot/README.md#the-tie) births a callable only
for a binder whose right-hand side is a callable shape at its root, or a
combined shape. A `FN` written anywhere else — a body's last statement, as
tutorial 04's `CONSTANTLY` returns one, the head of a call, an argument — already
has a callable shape: the [shape builder](../../src/scope/README.md#visibility)
builds it, classifies its captures and records its signature node by site. Only
`BodyShape::births` is limited to a binder's root, and nothing births the
function.

**Acceptance criteria.**

- A door births the callable at a site from its body shape and signature node,
  reading its captures through an activation view, as a one-node knot laid in
  the region its caller names.
- A `FN` whose body reads a name declared after it in the same body finds that
  name bound when it is born.
- A `FN` written as a body's last statement, born through the door, is returned
  from the call and called afterward with the captures it was born with.

**Directions.**

- *A `FN` inside a knot — deferred.* One under a data binder's constructor slot
  that captures a fellow member has to be born inside that knot, and stays
  [unplanned work](README.md#unplanned-work).

## Dependencies

**Requires:** none — foundation.

**Unblocks:**

- [Dispatch](dispatch.md) — the evaluator births a lambda where it meets one.
