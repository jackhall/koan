# Quotes resolve where they are written

A quote's free names reading the bindings visible where the quote is written,
wherever its code is evaluated.

**Problem.** The shape builder never enters a quote: its code is data until an
`EVAL` builds a block shape for it when the `EVAL` runs, and every free name
then resolves by name at the `EVAL`'s own position
([names that arrive at run time](../../src/scope/README.md#names-that-arrive-at-run-time)).
So `TWICE #(PRINT x)`, where `TWICE` runs its code argument with `$(…)`, reads
`x` in `TWICE`'s scope rather than the caller's, and a name `TWICE`'s scope
happens to declare captures the caller's. At run time that search reaches
through only one frame: [`through_chain`](../../src/scope/activation.rs)
follows a block activation's pointer to its enclosing one, and a callable's or
module's activation reaches only its own slots and the captures its shape fixed
where it was built, so reaching a defining frame's activation needs a hold on
that frame's cell, which nothing takes. And a statement containing `EVAL` at any
depth waits on every unit declared before it, so a binder before it that waits
on that statement refuses the body with `EvalCycle`.

**Acceptance criteria.**

- The shape builder builds a written quote as a shape of its own: its free names
  and keyworded uses resolve where the quote is written, to coordinates and
  candidate lists, and its value carries each resolved name's binding beside
  its symbol.
- `TWICE #(PRINT x)` prints the caller's `x`, whatever `TWICE`'s own scope
  declares.
- A binder the evaluated code declares never captures a name that resolved
  where its quote was written.
- A quote's mentions are deferred: it sees every binder in its body, as a
  callable body does, and a quote in a cycle is born in that cycle's knot as a
  member of its own.
- An `EVAL` of a written quote runs the shape built where the quote was written
  and lays no shape down.
- An `EVAL`'s operand is an eager mention like any other, so an `EVAL` inside
  its own quote's cycle is the ordinary eager-cycle refusal.
- Code equality follows bindings as a bisimulation: a quote equals its copy,
  `LET echo = #(PRINT echo)` compared with a copy of itself terminates equal,
  and two quotes of the same text whose names bind different values are
  unequal.
- Printing code never follows a binding.

**Directions.**

- *Quotes resolve where they are written — decided.* Bindings are immutable, so
  resolving a name at the quote is closing over its binding when the quote is
  born, as a callable does, and a quote's free names are written in it, so its
  captures are exact.
- *Quotes are knot members — decided.* A quote sees its whole body, as a lambda
  written in its place would. Kernel reaches the same visibility through a live
  reference to the caller's environment; koan holds no environment and copies
  bindings, so a cycle through a quote is a knot edge.
- *A name that does not resolve at its quote — open.* A free name bound nowhere
  at its quote, or a symbol built at run time, carries no binding. It can be an
  unbound-name error when evaluated, so no shape retains its defining scope, or
  resolve where the `EVAL` is written — a deliberate escape, like Racket's
  `datum->syntax` — which needs a hold on the `EVAL`'s frame and orders the
  `EVAL` after the binders declared before it.
- *When a written quote resolves — open.* Where it is written, when the
  program loads, as the first criterion states; or only where an explicit
  `RESOLVE` asks, so a program can assemble fragments before any name in them
  resolves.
- *Code equality — decided.* Code compares by structure and follows its
  bindings as a bisimulation, as circular data does
  ([equality](../../src/values/README.md#equality-and-rendering)). Ignoring
  bindings would make two quotes that evaluate differently equal, and comparing
  them by node identity would make a copy unequal to its source.
- *A binding that holds a function — open.* A function compares `Incomparable`
  ([equality and rendering](../../src/knot/README.md#equality-and-rendering)),
  and every called name and keyworded use binds one, so a bisimulation reaching
  a function makes nearly all code incomparable. The alternatives: accept that;
  pair function nodes by body shape and bisimilar captures, giving functions a
  structural equality inside code only; or compare a function binding by node
  identity, so code that calls a function is unequal to its copy.

## Dependencies

**Requires:**


**Unblocks:**

- [Dispatch](dispatch.md) — a quote's keyworded uses resolve where it is written.
- [Code names](code-names.md) — a sigiled name inside a quote resolves beside the quote's bindings.
- [Code splicing](code-splicing.md) — a quote's bindings and knot edges.
