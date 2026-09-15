# Function values

Functions as values, now that a scope exists for them to capture.

**Problem.** `values` ([src/values/README.md](../../src/values/README.md)) ships data
values, containers and working expressions, and no function arm: a function
value is its type handle plus the environment it captured, and the environment
is the scope layer's closure bindings
([src/scope/README.md](../../src/scope/README.md#three-tiers)), which `values`
may not name. The scope layer in turn ships the closure birth and the
component analysis but nothing that ties a component as a
[knot](../../src/memory/README.md#the-knot), and nothing elaborates a
signature's type expressions into a lattice handle. The old runtime's
[`KFunction`](../../src/machine/core/kfunction.rs) holds a bare `&Scope` into
the region it lives in, which is why it could never be copied; the rewrite's
closure bindings are value words and knot edges, so nothing forces that.

**Acceptance criteria.**

- `Value` has a callable arm over a parameter the function layer closes with a
  function node; the node is region-resident, `Drop`-free, born only through
  the tie, and reports its memoized type handle.
- A function's captured environment is the scope layer's closure bindings, and
  neither `values` nor `scope` gains a dependency edge on the function layer.
- A callable copies at a crossing by re-tying its knot at the destination, and
  is priced by the knot's memoized weight under the ordinary verdict.
- Structural equality over a callable is an error the `==` builtin reports,
  never `false`.
- A function's closure bindings are born from its body's shape, with the
  capture set the shape computes under the scopes' visibility predicate
  ([src/scope/README.md](../../src/scope/README.md#visibility)); a capture of
  a fellow component member is a knot edge.
- A deferred-only component whose members are all callable binders is born as
  one knot; one with a data member is refused with a diagnostic.
- A function's type is elaborated from its signature where the function is
  born: a `FN`'s from its parameter schema and return, an `EXPR`'s or `OP`'s
  as its expression shape, a `FOR ALL` name as a quantifier.
- Each new module's design doc is the `README.md` in its source directory, and
  the module's top-of-file comment links it.

**Directions.**

- *How `Value` names a callable — decided.* One arm over a type parameter,
  `Value<'graph, 'cell, X>`, with a vacuous default; `values` states what it
  asks of `X` as a trait pair (per-value: type, weight, render, sibling;
  per-family: the copy), `scope` threads the parameter through, and `function`
  closes it with a sixteen-byte knot member, so the value word stays at
  twenty-four bytes. `cellgraph`'s `reattachable!` macro gains a generic-family
  arm for the value family.
- *Every function is a knot node — decided.* A non-recursive function is a
  one-node knot: one representation, one birth path, one copy.
- *Callables copy — decided.* Closure bindings are value words and edges, so a
  copy re-ties the whole knot at the destination, deep-copying each binding and
  carrying each edge verbatim; the knot's weight is memoized on every node at
  the tie. The one shape that cannot copy, one that retains its defining scope
  for an `EVAL`, is [unplanned work](README.md#unplanned-work).
- *Which components this item ties — decided.* Components of callable binders
  only. A deferred-only component with a data member — a container holding a
  callable that captures it, or a ring of containers — is refused by the tie;
  [Circular values](circular-values.md) ties it.
- *Type expressions — decided.* A module of its own, `elaborate`, above `scope`
  and below `function`, covering bare names, `LIST OF`, `MAP … ->`, `FN :{…}
  -> R`, `EXPR (head) -> R` with and without `FOR ALL`, `A | B`, `:{…}` and
  `Union.Tag`. Constructor application is refused for [Modules](modules.md).
- *Reading an edge capture — decided.* An activation holds the callable it
  runs, and a read of an edge capture resolves through it to the sibling's
  value, so no reader ever sees a bare edge.
- *Mutual recursion — decided.* A callable body's mentions are deferred, so it
  sees every sibling in its scope in any source order
  ([src/scope/README.md](../../src/scope/README.md#visibility)); a strongly
  connected component of callable bindings is born together as one knot, with
  a closure binding that names a fellow member holding a knot edge.
- *Who ties — decided.* The function layer ties one component and hands the
  knot back; the caller binds each member's slot. A component is one unit of
  work that binds several slots, a contract on the scheduler.

## Dependencies

**Requires:** none — [scopes](../../src/scope/README.md) ship.

**Unblocks:**

- [Modules](modules.md) — a module value closes the same parameter beside the function node.
- [Circular values](circular-values.md) — a data knot extends the tie functions ship.
- [Dispatch](dispatch.md) — there is nothing to dispatch on until functions exist.
- [Yielding iterators](yielding-iterators.md) — a stream at rest holds a function value.
