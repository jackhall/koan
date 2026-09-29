# Calls solved from their static types

A keyworded call's group solved from what the load knows of its arguments, and
from what the run carries only where the load knows nothing.

**Problem.** A keyworded call of a quantified candidate solves its group from
the carried arguments, even where the load solved it from the arguments'
[static types](../../src/dispatch/README.md#static-types), which read the
declarations the carried ones cannot see. Under
`EXPR FOR ALL #[Elt] #(PAIR x :Elt WITH y :Elt) -> Str`, parameters
`a :(Number | Str)` and `b :(Number | Str)` holding `1` and `"x"` solve `Elt` to
`Number | Str` over their static types, but the call fixes `Elt` to `Number`
from `a`'s carried type and refuses `b`, so `PAIR a WITH b` is a no-overload
fault. The load cannot call such a candidate *always*, so the call admits it
again.

**Acceptance criteria.**

- Each argument of a keyworded call contributes to its group's solve the upper
  end of its static type, or its carried type where that upper end is `Any`.
- `PAIR a WITH b` above runs with `Elt` bound to `Number | Str`, and
  `PAIR a WITH e`, where `e` is an `EVAL` the load types `Any`, runs where `e`
  yields a `Str` and faults where it yields a `Bool`.
- A use none of whose arguments contributes its carried type is judged from its
  static solve alone: `PAIR a WITH b` is *always*, and selected at load as its
  lone candidate.
- A debug build checks that each argument a call binds this way carries a type
  under its slot at the solution.

**Directions.**

- *A static solution may change what a call binds — decided.* The static solve
  reads declarations and ascriptions, so it is the more likely to be right; "a
  call runs what selection over the full list would" gives way where the two
  differ.
- *What an argument contributes — decided.* Per argument, what the load knows
  of it, and what the run carries where the load knows nothing, so a declared
  argument and an `EVAL`'s complement each other in one solve. An argument at
  most `Any` is one the load knows nothing of, a parameter declared `Any` among
  them, since `Any` constrains nothing; a programmer moves an argument to the
  load's side with [`:!`](value-ascription.md).
- *A contribution holding a lexical variable — open.* An argument typed by an
  enclosing body's `Elt` contributes `Elt`, which the run replaces through a
  coordinate the use's body reads, and a body that never names `Elt` captures
  nothing to read it through. The shape builder can capture every lexical
  variable a contribution reaches, or such an argument can contribute its
  carried type, which may bind `Number` where the enclosing call bound
  `Number | Str`. Relying on the load wherever it can favours capture.
- *A call by name — open.* The frame solves a call by name from the record it
  is handed, which carries no type parameter; contributing a static type needs
  a channel into it, or leaves calls by name to their carried solve.

## Dependencies

**Requires:**

- [Static types of generic code](solving-from-static-types.md) — the static
  solve and its verdicts.
- [Value ascription](value-ascription.md) — an ascribed argument's static type.

**Unblocks:** none — a leaf.
