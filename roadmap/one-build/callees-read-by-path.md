# A callee read by a member path or a call's result

A call whose callee is a module's member, or another call's result called in
place, given a `PassesName` edge, so the load makes its instance.

**Problem.** A call passes its callee's names a `PassesName` edge only where
the text names the callee: a keyworded use, or a call by a name the load
types through to a callable body
([edges](../../design/binding-graph.md#edges)). A returned `FN` bound by a
`LET` and called by name is such a name, and the load makes its instance. A
callee read by a member path, off a written module or a functor's result
alike, and a call's result called in place get no edge. A run cell makes the
callee's instance, and a refusal the text decides faults at run where every
named callee's refuses the load ([refusal](../../src/build/README.md#refusal)).
Under

```koan
SIG Counter FOR ALL #[Carrier] = #[(VAL zero :Carrier)]
MODULE ints = ((LET zero = 0))
MODULE strs = ((LET zero = "a"))
EXPR #(NEEDS n :Number) -> Number = #(n)
EXPR #(USE m :Counter) -> Any = #(NEEDS m.zero)
EXPR #(MAKER m :Counter) -> :(FN :{k :Counter} -> Any) = #(
  FN :{k :Counter} -> Any = #(USE k)
)
PRINT "start"
```

`USE strs` refuses, since `strs.zero` is a `Str`. This refuses the load and
prints nothing:

```koan
LET g = (MAKER ints)
PRINT (g {k = strs})
```

Each of these prints `start` and then faults:

```koan
MODULE held = ((LET g = (FN :{k :Counter} -> Any = #(USE k))))
PRINT (held.g {k = strs})
```

```koan
EXPR #(BUILDER m :Counter) -> Module = #(
  MODULE made OVER #[m] = ((LET g = (FN :{k :Counter} -> Any = #(USE k))))
)
LET built = (BUILDER ints)
PRINT (built.g {k = strs})
```

```koan
PRINT ((MAKER ints) {k = strs})
```

**Acceptance criteria.**

- A call whose callee is read by a member path carries a `PassesName` edge
  to the member's body, for a member of a written module and of a functor's
  result alike, and the load makes the callee's instance where it makes the
  caller's.
- A call's result called in place carries the same edge to the body the
  call returns.
- Each of the three faulting programs above refuses the load and prints
  nothing.
- A test checks each of the three, and `(g {k = strs})` after
  `LET g = (MAKER ints)` beside them.

**Directions.**

- *Where a built callee's body is recorded — open.* On what the load knows
  of the value the builder returns — a module's closed content tree, a
  returned function's — so every reader of it sees the body; or the member
  read and the in-place result are holders, whose arrival names the body
  ([the binding graph's text](../../src/build/README.md#the-binding-graphs-text)).

## Dependencies

**Requires:** none.

**Unblocks:** none.
