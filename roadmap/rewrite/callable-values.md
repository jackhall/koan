# Callable values

Functions and modules as values, once a scope exists for them to capture.

**Problem.** `values` ([src/values/README.md](../../src/values/README.md)) ships data
values, containers and working expressions, and no function or module arm: a
callable's value is its type handle plus the environment it captured, and the
environment is a scope, which `values` may not name. The old runtime's
[`KFunction`](../../src/machine/core/kfunction.rs) and
[`Module`](../../src/machine/model/values/module.rs) hold a bare `&Scope`
into the region the callable lives in, so a function value cannot exist before
the scope layer does, and a scope's binding table cannot hold one until the
value exists.

**Acceptance criteria.**

- `Value` has a function arm and a module arm; each is region-resident,
  `Drop`-free and born through a brand-confined door, and each reports its
  memoized type handle (a function type; a module's self-signature).
- A callable's captured environment is the scope layer's type, and neither
  `values` nor `scope` gains a dependency edge on the other to hold it.
- A value containing a callable prices uncopyable at the crossing verb, so a
  crossing pins it; a forced copy of one is a refused crossing, not a panic.
- Structural equality over a callable is an error the `==` builtin reports,
  never `false`.
- A function value's closure bindings are born from its body's shape, and the
  capture set the shape computes uses the scopes' visibility predicate
  ([src/scope/README.md](../../src/scope/README.md#visibility)).

**Directions.**

- *How `Value` names the environment — open.* A `Reattachable` family
  parameter on the value vocabulary that the scope layer instantiates
  (requires a generic arm on `cellgraph`'s `reattachable!` macro), or the scope
  layer owning the two arms over a `Value` extension point of its own.
  Recommended: decide against the scope layer's binding-table shape once it
  stands.
- *Capture into closure bindings — decided.* A closure's bindings are a
  shallow copy of the names its body reads from enclosing scopes, born once
  none of them is a placeholder, per
  [src/scope/README.md](../../src/scope/README.md#three-tiers).
- *Whether a callable prices uncopyable — open.* Closure bindings hold value
  words, and every data value has a deep copy, so a closure over data alone
  could copy at a crossing; one over another callable reaches the same
  question recursively. Recommended: settle it against the binding shape the
  scope layer ships.
- *Mutual recursion — open.* A body sees no later sibling of its definition,
  so two functions that call each other need a definition window — implicit,
  or a module body, as co-declared types have. The window's functions are born
  together as one [knot](../../src/memory/README.md#the-knot), since a closure's bindings copy only
  once nothing they copy is pending; which siblings form the knot is the
  window's to compute. The definition-site cutoff is itself provisional until
  this is settled.
- *`USING … SCOPE` over a module — decided.* The surfaced names come from the
  module's signature, which must be known statically at the `USING` site; a
  module whose signature is not requires an ascription there.

## Dependencies

**Requires:**

- [Scope on values and types](scope-on-values-and-types.md) — a callable captures a scope.

**Unblocks:**

- [Dispatch](dispatch.md) — there is nothing to dispatch on until functions exist.
- [Yielding iterators](yielding-iterators.md) — a stream at rest holds a function value.
