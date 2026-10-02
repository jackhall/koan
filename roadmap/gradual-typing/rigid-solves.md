# Solves over a lexical variable

A load-time solve that each run reproduces where an argument's static type names
a type the run binds.

**Problem.** The unifier reads a lexical variable as one type: rigid, an opaque
type related to others only through its ends, or through its bound. That reading
answers what holds at every binding, which is what *fits* and a verdict ask. It
does not answer what a run computes at its binding, which is what a solve the
call reproduces must give. The solver's meet (`meet_through_variables`) gives
`Never` for `Number` against an unrelated `Outer`, where a run binding `Outer`
to `Number` gets `Number`; `admits` admits a carried `Outer` whose bound is a
union through the bound's members, contributing them rather than `Outer`. A
variable stands for every type between its ends, and both read it as a single
type.

So the load never takes a class solve over a static type holding a lexical
variable as the call's own ([`judge_by_class`](../../src/type_lattice/ranking.rs)),
and an instance argument whose variables an earlier class solves from such a
type is refused (`ShapeError::Unfixed`). Under `EXPR #(APPLY 2 TO 1)` and
`EXPR FOR ALL #[Elt] #(APPLY f :(FN :{x :Elt} -> Elt) TO y :Elt) -> Elt`, in the
body of an `EXPR FOR ALL #[Outer]` with a parameter `xs :(LIST OF Outer)`,
`APPLY pick TO xs` refuses the load.

**Acceptance criteria.**

- A law states that where the load takes a solve over lexical variables as the
  call's own, binding each variable as a run does and then solving gives the
  load's solution so bound.
- `APPLY pick TO xs` above loads, and instantiates `pick` at
  `Elt = LIST OF Outer` over each run's binding of `Outer`.
- A class whose solving arguments are exact and name a lexical variable is
  judged by its static solve where that solve is the call's own, as a closed
  one is.

**Directions.**

- *What the lattice answers — open.* A solve door over parametric
  contributions can report, per variable, whether its solution is the same
  type at every binding — joined from lower contributions none of which was
  read through a bound — and refuse the rest; or the solver's operations can
  answer an interval for a variable where they now pick one type. Recommended:
  the report, since it leaves every other reader of the solver as it is.

## Dependencies

**Requires:**

- [Instances over a lexical variable](open-instances.md) — records a rigid instance solution.

**Unblocks:** none — a leaf.
