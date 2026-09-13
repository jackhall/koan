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
- The old runtime's tutorial programs that define functions and modules run
  on the rewritten stack and print the same output.

**Directions.**

- *How `Value` names the environment — open.* A `Reattachable` family
  parameter on the value vocabulary that the scope layer instantiates
  (requires a generic arm on `cellgraph`'s `reattachable!` macro), or the scope
  layer owning the two arms over a `Value` extension point of its own.
  Recommended: decide against the scope layer's binding-table shape once it
  stands.
- *By-reference capture — decided.* A closure captures its scope, per
  [lazy-closures.md](../../old_design/lazy-closures.md); by-value capture is
  not on the table.

## Dependencies

**Requires:**

- [Scope on values and types](scope-on-values-and-types.md) — a callable captures a scope.

**Unblocks:** none tracked yet.
