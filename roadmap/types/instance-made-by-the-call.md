# An instance made by the call

An instance site whose group only the call can solve, instantiated by that
call.

**Problem.** A quantified function passed as an argument is instantiated at
its candidate's slot, read through the group the call's other arguments solve
([instance sites](../../src/dispatch/README.md#static-types)). An argument
whose static type is at most `Any` contributes the type its value carries, so
where it sits at a slot that solves, only the call knows the solution, and the
load refuses the instance site as `Unfixed`. Under

```koan
EXPR #(APPLY 2 TO 1)
EXPR FOR ALL #[Elt] #(APPLY f :(FN :{x :Elt} -> Elt) TO y :Elt) -> Elt = #(
  f {x = y}
)
MODULE lib = (
  LET pick = (FN FOR ALL #[Elt] :{x :Elt} -> Elt = #(x))
  EXPR #(WRAP a :Any) -> Any = #(
    APPLY pick TO a
  )
)
```

every call of `WRAP` fixes `Elt` by what `a` carries, yet the load reports
"nothing fixes `Elt` here". A program gets past it only by naming one type,
`APPLY pick TO (a :! Number)`, where the body is meant for any.

**Acceptance criteria.**

- The program above loads. With `PRINT (WRAP 1)` and `PRINT (WRAP "s")` in
  the module body it prints `1` and `s`: each call instantiates `pick` at the
  type `a` carries.
- The call makes the instance once the classes before its slot's are solved,
  through the door the load's instance sites solve a class through, and binds
  a function stamped with the concrete type its group was solved at.
- A call by name whose other fields contribute the types their values carry
  instantiates its instance field the same way.
- Where the candidates a call keeps want different instances, the call faults
  as the load refuses them (`AmbiguousInstance`). A candidate whose slot gives
  the scheme no instance does not admit.
- The instance a call makes equals the one the load makes with each such
  argument's carried type declared, over generated programs and arguments.
- An instance site nothing wants at a type, one wanted at a union or `Any`,
  and one whose solving argument is itself an instance site are refused at
  the load.

**Directions.**

- *Who makes the instance — decided.* The call, where a solving argument
  contributes its carried type, and the build everywhere else
  ([where a solve is decided](../../design/binding-graph.md#where-a-solve-is-decided)).
- *Candidates that want different instances — decided.* The call faults as
  the load refuses them, since
  [the load and the run agree](../../design/gradual-typing.md#the-load-and-the-run-agree).
- *A scheme the load cannot see into — decided.* Not this item. A read whose
  operand the load cannot see into is no instance site, and faults where it
  reads a scheme anywhere but a call's head.
- *What the load records for the site — open.* The scheme and each
  candidate's slot, kept in the unit record for the call. Or, once the build
  runs over the binding graph, the instance as a `SiteQuestion` waiting on a
  name declared `Any`, which the run cell that reaches it answers
  ([what a bind site binds](../../design/binding-graph.md#what-a-bind-site-binds)).

## Dependencies

**Requires:** none.

**Unblocks:** none — a leaf.
