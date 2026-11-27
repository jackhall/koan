# A module read out of a nominal parameter

A call's mapping written past a record or union parameter to the module it
holds.

**Problem.** Under
[a module passed through an alias, a capture, a view or a member read](captures-and-views-traced.md)
a content tree's leaf is load content or a name of a body, and only a type
name or a module binder is a name of the graph
([what a bind site binds](../../design/binding-graph.md#what-a-bind-site-binds)):
a record or union parameter binds none, and a record value is not load
content. So a call that reads a module out of such a parameter maps nothing:
a run cell makes its callee's instance, and a refusal under it faults at run.
In this program, which prints `14`, `(CARRIER xs.head)` is such a call:

```koan
SIG Counter FOR ALL #[Carrier] = #[(VAL zero :Carrier) (VAL succ :(FN :{x :Carrier} -> Carrier))]
MODULE ints = ((LET zero = 0) (LET succ = (FN :{x :Number} -> Number = #(x + 1))))
NEWTYPE Chain = :{head :Counter, tail :(Chain | Null)}
EXPR #(CARRIER m :Counter) -> Number = #((LET v = (m :| Counter)) (7))
EXPR #(COUNT xs :Null) -> Number = #(0)
EXPR #(COUNT xs :Chain) -> Number = #((CARRIER xs.head) + (COUNT xs.tail))
LET two = (Chain {head = ints, tail = (Chain {head = ints, tail = null})})
PRINT (COUNT two)
```

And this one prints `start` before it refuses, though its text fixes the
module `USE` takes:

```koan
SIG Holder FOR ALL #[Carrier] = #[(VAL zero :Carrier)]
MODULE strs = ((LET zero = "a"))
EXPR #(NEEDS n :Number) -> Number = #(n)
EXPR #(USE m :Holder) -> Any = #(NEEDS m.zero)
EXPR #(GO rr :{c :Holder}) -> Any = #(USE rr.c)
PRINT "start"
PRINT (GO {c = strs})
```

**Acceptance criteria.**

- In the first program, the load makes `CARRIER`'s instance for
  `(CARRIER xs.head)`, and the instances `CARRIER`'s other callers reach are
  unchanged.
- The second program refuses the load and prints nothing.
- The property laws hold unchanged
  ([properties](../../src/build/README.md#properties)).

**Directions.**

- *What the parameter binds — open.* A record or union parameter binds a
  name for the content of the module parts its sites read, a projection
  rather than its whole digest; or a record or union value built over load
  content becomes load content, named by its digest. Recommended: the
  projection, since a bind site then files a view only on what its
  questions read.

## Dependencies

**Requires:**

- [A module passed through an alias, a capture, a view or a member read](captures-and-views-traced.md) — the content tree this extends.

**Unblocks:** none.
