# Exact types from the retype

The load's use of what the run's retype makes exact, past `:!` and parameters.

**Problem.** The run retypes a value to its declared type at a `:!`, a
parameter and a frame's return
([retype](../../src/values/README.md#the-type-memo-and-satisfies)), but the load
reads only the first two as exact, and only at a list, dict or record type
([static types](../../src/dispatch/README.md#static-types)):

- A call is at most its callee's substituted return, though a spawned frame
  yields a value carrying exactly that return where it is a list, dict or
  record type.
- A candidate is *never* only where a slot meets its argument's upper end at
  `Never`, so an exact argument a slot does not admit leaves it *maybe*. Beside
  `EXPR #(WHICH x :(LIST OF Number)) -> Str = #("numbers")` and
  `EXPR #(WHICH x :(LIST OF Any)) -> Str = #("any")`, the use `WHICH x` in
  `EXPR #(SHOW x :(LIST OF Any)) -> Str = #(WHICH x)` keeps both candidates, and
  every call of `SHOW` admits and ranks them.
- A `:!` or a parameter at a newtype or a family application is at most its
  type, though the run retypes a tagged value to the application it lies under.

**Acceptance criteria.**

- `WHICH x` in `SHOW` above is selected at load, to the `LIST OF Any` overload.
- A keyworded call the load selects an unquantified candidate for, or a
  quantified one whose group's intervals are points, and a call by name of an
  unquantified function, are exactly the callee's substituted return where that
  is a list, dict or record type.
- A `:!` or a parameter at a newtype or a family application is exactly its
  type; one at a union keeps a variant's own type, and stays at most the union.

**Directions.**

- *Never over an exact argument — open.* `judge_by_class` would call a slot
  that does not admit an exact argument *never*: a
  [type lattice](../../src/type_lattice/README.md) change, which needs the
  user's approval before it is planned.
- *A call in tail position — open.* Its node never finishes: the frame it tails
  into carries the enclosing contract's return, so "exact at the callee's
  return" is false there, though nothing reads the node's value. The load can
  type it exact anyway, or at most its return.

## Dependencies

**Requires:** none.

**Unblocks:** none — a leaf.
