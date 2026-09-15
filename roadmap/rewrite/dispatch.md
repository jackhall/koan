# Dispatch

Keyword lookup over scopes: choosing the callable a keyworded expression runs,
and binding its arguments.

**Problem.** The old runtime's dispatch keeps its registrations in the keyed
channel of [machine/core/bindings.rs](../../src/machine/core/bindings.rs), and
[`resolve_dispatch`](../../src/machine/execute/decide/resolve_dispatch.rs)
walks the scope chain to a `Resolved`, `Ambiguous`, `ParkOnProducers`,
`UnboundName` or `Unmatched` outcome. Builtins — `LET` and `PRINT` included —
register as functions through
[`register_builtin`](../../src/builtins.rs), so even a program that declares
no function dispatches on every statement, and a reference to a binder that
has dispatched but not yet bound parks through name placeholders
([name-placeholders.md](../../old_design/execution/name-placeholders.md)). The
rewrite's [scopes](../../src/scope/README.md) resolve value and type names
only: nothing selects a callable for a keyworded expression, so no koan
program runs on the rewritten stack.

**Acceptance criteria.**

- Dispatch keys a keyworded registration on its full bucket key, and a typed
  argument that does not satisfy a candidate is a non-match that falls through
  rather than a bind-time error.
- An overload is a candidate only where the scopes' visibility predicate admits
  it.
- A builtin bucket is unshadowable: a user registration whose untyped bucket
  key collides with a builtin's is rejected.
- A reference to a visible binder that has not yet bound parks until it binds,
  and a dispatch placeholder keys on the full bucket key.
- The old runtime's tutorial programs that use no feature beyond values,
  scopes and functions run on the rewritten stack and print the same output,
  and `tools/verify_snippets.py` reads the rewritten binary.
- The module's design doc is the `README.md` in its source directory, and the
  module's top-of-file comment links it.

**Directions.**

- *Builtins as function values — decided.* A builtin registers through the same
  bucket a user function does, as a function value whose body is native.
- *Newtype construction — decided.* An ordinary construction `(Head payload)`
  is `Tagged::construct`, the one construction rule the tie checks a knot's
  tagged nodes by too ([src/values/README.md](../../src/values/README.md#what-a-value-is)).
- *Keyword reads resolved in the shape — open.* Builtin buckets are
  unshadowable, so a builtin form resolves when the shape is built; a user
  bucket is shadowable and overloaded. Recommended: extend the shape's
  coordinate resolution to bucket keys, as value and type names resolve
  ([src/scope/README.md](../../src/scope/README.md#resolution)).

## Dependencies

**Requires:**

- [Scheduler on cellgraph](scheduler-on-cellgraph.md) — running a program needs the scheduler that drives it.

**Unblocks:**

- [Modules](modules.md) — a module program runs only under dispatch.
- [Yielding iterators](yielding-iterators.md) — a demand for an element is an ordinary dispatch.
