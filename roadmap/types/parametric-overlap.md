# The overlap check over a parametric registration

A user overload a builtin always beats, refused at load whether its signature
is closed or quantified.

**Problem.** [The overlap check](../../src/dispatch/README.md#the-overlap-check)
reads a registration's load-time expression shape, and checks a closed or a
rigid one: a rigid one's check is a question each binding answers
([resumption](../../src/build/README.md#resumption)). A quantified one is
never checked: `EXPR FOR ALL #[Elt] #(x :Elt + y :Elt) -> Str = #("mine")`
loads beside the builtin `+`, which wins every call the two share:
`PRINT (1 + 2)` prints `3`.

**Acceptance criteria.**

- The quantified `+` above refuses the load with `ShapeError::Overlaps`, as a
  closed overload meeting the builtin `+` does.
- A quantified registration no binding within its group's bounds makes
  overlap a builtin loads: `EXPR FOR ALL #{Elt: Str} #(x :Elt - y :Elt) -> Str`
  beside the builtin `-` over `Number`.

**Directions.**

- *Which bindings a quantified registration is checked at — open.* Refused
  where some binding within its group's bounds makes every slot pair meet a
  builtin overload's above `Never`, the closed rule read over the bounds; or
  only where every binding does. Recommended: some binding, which the `+`
  criterion assumes, since the builtin wins every call at that binding.

## Dependencies

**Requires:** none.

**Unblocks:** none — a leaf.
