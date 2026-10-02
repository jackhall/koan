# Instances over a lexical variable

A quantified function instantiated at a type each run binds, or at a type the
arguments ranked before it solve.

**Problem.** An [instance site](../../src/dispatch/README.md#static-types)
whose solution names a lexical variable refuses the load
(`ShapeError::OpenInstance`). In a module body binding
`LET pick = (FN FOR ALL #[Elt] :{x :Elt} -> Elt = #(x))`,
`EXPR FOR ALL #[Outer] #(WRAP a :Outer) -> :(FN :{x :Outer} -> Outer) = #(pick)`
wants `pick` at a type that solves `Elt` to `Outer`, which each call of `WRAP`
binds anew. The load records a solution as closed types, and the
[instance door](../../src/knot/README.md#an-instance) takes the one the load
made.

A keyworded argument that is an instance site is wanted at its slot read
through the group its callee's other arguments solve, and a variable the slot
shares with them must come out one closed point whatever the slots' priority
classes. Under `EXPR #(APPLY 2 TO 1)` and
`EXPR FOR ALL #[Elt] #(APPLY f :(FN :{x :Elt} -> Elt) TO y :Elt) -> Elt`,
`APPLY pick TO b` over a parameter `b :(Number | Str)` refuses the load
(`ShapeError::Unfixed`), though the call solves `Elt` from `b`'s static type
before it reads `f`.

**Acceptance criteria.**

- An instance site whose solution names a lexical variable loads, its static
  type exactly the instance over that variable. `WRAP 1` above returns a
  `FN :{x :Number} -> Number`, and `WRAP "s"` a `FN :{x :Str} -> Str`.
- The site reads the variable in a callable nested in the body that declares
  it as in that body itself.
- Two instances of one site made under one binding are equal, and under two
  bindings unequal.
- An instance argument of a closed candidate takes each variable of its slot
  that an earlier class solves from that class's solving slots, each at its
  argument's closed contribution, as the call solves it. `APPLY pick TO b`
  above loads and instantiates `pick` at `Elt = Number | Str`; so does the
  written-order `#(WITH y :Elt DO f :(FN :{x :Elt} -> Elt))` over `WITH b DO pick`.
- A variable the instance argument's own class solves keeps the closed-point
  rule: under `EXPR #(MAP 2 AT 1)` and
  `EXPR FOR ALL #[Elt Out] #(MAP f :(FN :{x :Elt} -> Out) AT y :Elt) -> Out`,
  `MAP pick AT b` loads with `Elt` from `b` and `Out` read through its bound,
  and the written-order `MAP pick AT b` stays refused.

**Directions.**

- *Where the solution is substituted — decided.* The load records a rigid
  solution beside the coordinates its variables are read at, and the evaluator
  substitutes it where the name is read or the literal is born, so nothing is
  solved at run time.
- *An earlier class solved over a rigid contribution — deferred* to
  [solves over a lexical variable](rigid-solves.md): such a solve is not the
  run's at every binding, so `APPLY pick TO xs` over `xs :(LIST OF Outer)`
  stays refused.

## Dependencies

Builds on the [type captures](../../src/scope/README.md#resolution) a call's
contribution reads a lexical variable through.

**Requires:** none.

**Unblocks:**

- [Solves over a lexical variable](rigid-solves.md) — reuses the rigid instance solution.
