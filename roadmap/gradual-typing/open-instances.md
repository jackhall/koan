# Instances over a lexical variable

A quantified function instantiated at a type each run binds.

**Problem.** An [instance site](../../src/dispatch/README.md#static-types)
whose solution names a lexical variable refuses the load
(`ShapeError::OpenInstance`). In a module body binding
`LET pick = (FN FOR ALL #[Elt] :{x :Elt} -> Elt = #(x))`,
`EXPR FOR ALL #[Outer] #(WRAP a :Outer) -> :(FN :{x :Outer} -> Outer) = #(pick)`
wants `pick` at a type that solves `Elt` to `Outer`, which each call of `WRAP`
binds anew. The load records a solution as closed types, and the
[instance door](../../src/knot/README.md#an-instance) takes the one the load
made.

**Acceptance criteria.**

- An instance site whose solution names a lexical variable loads, its static
  type exactly the instance over that variable. `WRAP 1` above returns a
  `FN :{x :Number} -> Number`, and `WRAP "s"` a `FN :{x :Str} -> Str`.
- The site reads the variable in a callable nested in the body that declares
  it as in that body itself.
- Two instances of one site made under one binding are equal, and under two
  bindings unequal.

**Directions.**

- *Where the solution is substituted — open.* The load can record a rigid
  solution beside the coordinates its variables are read at, and the evaluator
  substitute it where the name is read or the literal is born, so the solve
  stays at load. Recommended: this one, since nothing is then solved at run
  time.

## Dependencies

**Requires:**

- [Calls solved from their static types](static-solutions.md) — the type captures a body that never names the variable reads it through.

**Unblocks:** none — a leaf.
