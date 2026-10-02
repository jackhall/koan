# Calls solved from their static types

A call's group solved from what the load knows of its arguments, and from what
the run carries only where the load knows nothing.

**Problem.** A keyworded call of a quantified candidate solves its group from
the carried arguments, even where the load solved it from the arguments'
[static types](../../src/dispatch/README.md#static-types), which read the
declarations the carried ones cannot see. Under
`EXPR FOR ALL #[Elt] #(PAIR x :Elt WITH y :Elt) -> Str`, parameters
`a :(Number | Str)` and `b :(Number | Str)` holding `1` and `"x"` solve `Elt` to
`Number | Str` over their static types, but the call fixes `Elt` to `Number`
from `a`'s carried type and refuses `b`, so `PAIR a WITH b` is a no-overload
fault. The load cannot call such a candidate *always*, so the call admits it
again. A call by name solves from the carried types too, so what it binds and
returns follows what each run holds: `pair {x = a, y = b}` under
`FN FOR ALL #[Elt] :{x :Elt, y :Elt} -> :(LIST OF Elt)` returns a
`LIST OF Number` at one run and a `LIST OF (Number | Str)` at another, and the
load types it only at most.

**Acceptance criteria.**

- An argument of a keyworded call whose slot names a variable the slot's own
  [priority class](../../src/type_lattice/solving.md#priority-classes) solves
  contributes to that solve the upper end of its static type, or its carried
  type where that upper end is `Any`. Every other slot admits its argument's
  carried type.
- `PAIR a WITH b` above runs with `Elt` bound to `Number | Str`.
  `PAIR a WITH e`, where `e` is an `EVAL` declared `-> Any`, runs where `e`
  yields a `Str` and faults where it yields a `Bool`, and `PAIR e WITH a` runs
  where `e` and `a` both hold a `Str`.
- A use none of whose arguments contributes its carried type is judged from its
  static solve alone. `PAIR a WITH b` is *always*, and selected at load as its
  lone candidate. Under `EXPR FOR ALL #[Elt] #(FIRST xs :(LIST OF Elt)) -> Elt`,
  `FIRST m` over a parameter `m :((LIST OF Number) | Null)` is *never*, and
  refuses the load.
- A contribution holding a lexical variable is read, where the call runs, at
  the type the run binds the variable to, in a callable nested in the body that
  declares the variable as in that body itself. Under
  `EXPR FOR ALL #[Outer] #(BOTH a :Outer AND b :Outer) -> Str`, whose body binds
  `LET go = (FN :{} -> Str = #(PAIR a WITH b))` and returns `go {}`,
  `BOTH p AND q` over `p :(Number | Str)` and `q :(Number | Str)` holding `1`
  and `"x"` runs.
- A callable captures a type name its body does not write only where one of its
  calls contributes it: `go` above born under two bindings of `Outer` is two
  unequal functions, and a `FN :{} -> Str = #("hi")` in its place is one.
- A call by name whose callee the load reads as a quantified function solves
  its group from its argument record's static type, each field contributing as
  a keyworded argument does. `pair {x = a, y = b}` above is exactly
  `LIST OF (Number | Str)` whatever `a` and `b` hold, and `only {x = a}` under
  `FN FOR ALL #{Elt: Number} :{x :Elt} -> Elt` refuses the load.
- A debug build checks that each argument a call binds carries a type under its
  slot or parameter at the solution.

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
  load's side with [`:!`](../../src/dispatch/README.md#what-a-node-is).
- *Which slots read a contribution — decided.* Only a slot naming a variable
  its own class solves. A slot a later class admits against that solution reads
  the carried type, as an unquantified slot does.
- *A static type that cannot fit its slot — decided.* It contributes all the
  same, so the solve fails at every run and the load refuses the use, where an
  unquantified slot of the same type would be *maybe*.
- *A contribution holding a lexical variable — decided.* The static pass
  records, per callable, each type capture its calls' contributions reach,
  beside the captures the shape builder laid down, so a closure pays for one
  only where it reads it and compares unequal under two bindings only where
  they can change what it does.
- *A call by name — decided.* It solves from its argument's static type as a
  keyworded call does, through a channel from the call into the frame, so what
  it binds and returns follows the declarations and the load can refuse it.
- *An instance whose solution names a lexical variable — deferred.* To
  [instances over a lexical variable](open-instances.md), which builds on the
  capture this item lays down.

## Dependencies

**Requires:** none.

**Unblocks:**

- [Instances over a lexical variable](open-instances.md) — reads a lexical variable through the type captures this item records.
