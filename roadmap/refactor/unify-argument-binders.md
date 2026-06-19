# Unify the two argument binders

One argument-binding path instead of `bind` (which builds a whole `KFuture` to
produce a `Record<ArgValue>`) beside `bind_by_name` (which produces a
`Record<Carried>` directly).

**Problem.** A call's already-resolved arguments are bound to a function's
parameters by two separate methods that yield two different arg-record currencies:

- [`KFunction::bind`](../../src/machine/core/kfunction.rs) builds a whole `KFuture`
  (the parsed expr, the function, and a `Record<ArgValue>` whose parts are
  `resolve_for`-wrapped against each declared type). The builtin dispatch path
  ([`exec.rs`](../../src/machine/execute/dispatch/exec.rs)) calls it only to take
  `future.args` and discard the rest of the `KFuture`.
- [`KFunction::bind_by_name`](../../src/machine/core/kfunction/bind_by_name.rs)
  produces a `Record<Carried>` directly — a pure rename map with no `ArgValue`
  wrapping and no per-argument type-check (the picker already validated; the carried
  type is trusted). It is the binder the `exec` body executor uses for user-defined
  calls.

So a builtin call binds through `bind` (constructing a `KFuture` it immediately
guts), while a user-defined call binds through `bind_by_name`; the two arg
currencies (`Record<ArgValue>` versus `Record<Carried>`) coexist for the same
conceptual step. `KFuture` itself has an independent role as a lift carrier — the
waste is the arg-binding round-trip, not the future type.

**Acceptance criteria.**

- The builtin dispatch path obtains its argument record from a direct arg-binding
  call, not by constructing a `KFuture` and discarding every field but `args`.
- `KFunction::bind`'s `KFuture` construction is reached only by callers that consume
  the whole future (its lift-carrier role), never as an arg-binding shim.
- The two binders either share one argument-walk core, or the remaining
  `ArgValue`/`Carried` split is justified by a documented currency difference
  (`ArgValue` carries the `resolve_for` target type that `Carried` drops), not by
  "builtin call versus user-defined call."

**Directions.**

- *Keep `KFuture` — decided.* `KFuture` retains its independent lift-carrier role;
  the target is the arg-binding round-trip, not the future type.
- *Bind builtin args directly vs converge the currencies — open.* Either (a) add a
  direct arg-binding method that produces the builtin path's `Record<ArgValue>`
  without a `KFuture`, leaving the two currencies but removing the gutted-future
  round-trip; or (b) converge `ArgValue` and `Carried` so both binders share one
  record type and one arg walk. Recommended: (a) first — it removes the waste with a
  local change and surfaces whether (b) is even possible, since `ArgValue` carries
  the per-argument `resolve_for` target type that `Carried` does not.

## Dependencies

An engine-internal dispatch-path hygiene item; update
[design/execution-model.md](../../design/execution-model.md) if the argument-binding
vocabulary it names changes.

**Requires:** none — engine-internal.

**Unblocks:** none tracked yet.
