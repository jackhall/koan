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
  `Never`; the argument's lower end is never read. So an exact argument a slot
  does not admit leaves it *maybe*. Beside
  `EXPR #(WHICH x :(LIST OF Number)) -> Str = #("numbers")` and
  `EXPR #(WHICH x :(LIST OF Any)) -> Str = #("any")`, the use `WHICH x` in
  `EXPR #(SHOW x :(LIST OF Any)) -> Str = #(WHICH x)` keeps both candidates, and
  every call of `SHOW` admits and ranks them.
- A `:!` or a parameter at a newtype, a union's variant, a family or a family
  application is at most its type, though the run retypes a tagged value to the
  one it lies under.

**Acceptance criteria.**

- `WHICH x` in `SHOW` above is selected at load, to the `LIST OF Any` overload.
- A candidate one of whose slots, read at its greatest instance, does not lie
  above its argument's lower end is *never*: after `LET r = {v = 1}` and
  `LET rec = {a = r.v}`, a use `GET rec` whose only candidate takes
  `x :{b :Number}` refuses the load.
- A keyworded call the load selects a candidate for, and a call by name of a
  callee whose static type is exact, are exactly the callee's substituted
  return where their group is unquantified or its intervals are points, and the
  retype makes that return exact: a list, dict or record type, a family or its
  application, a newtype or a union's variant.
- A test pins that no builtin declares a return the retype makes exact, so a
  selected builtin's call is exact with no native retyped.
- A `:!` or a parameter at a newtype, a union's variant, a family or a family
  application is exactly its type; one at a union keeps a variant's own type,
  and stays at most the union.

**Directions.**

- *Never over an argument's lower end — decided.* A
  [type lattice](../../src/type_lattice/README.md) change the user approved:
  `judge_by_class` reads each argument's lower end, below its rigid variables,
  against the slot's greatest instance. The run's carried type lies above the
  lower end, so a slot that does not lie above it admits no call. It covers
  every exact argument, and an inexact one with a lower end.
- *A call in tail position — decided.* It is typed as any other call. Its node
  never finishes: the frame it tails into returns at the enclosing contract, so
  no carried type contradicts it, and the return check reads it unchanged.
- *A builtin's return — decided.* A test over the builtin table, not a retype at
  each call: the run pays nothing, and a builtin declaring a list, dict, record
  or nominal return fails the test.

## Dependencies

**Requires:** none.

**Unblocks:** none — a leaf.
