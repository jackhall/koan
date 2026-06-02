# Seed every scope with builtins to skip the root walk

Make the builtins reachable at every scope instead of only the root, so the
hottest lookups stop walking the scope chain — and so the no-shadow invariant for
type builtins falls out of the ordinary rebind machinery.

**Problem.** Builtins register only into the run-root scope
([`src/builtins.rs`](../../src/builtins.rs) `default_scope`). A scope's
[`Bindings`](../../src/machine/core/bindings.rs) is eight `RefCell<HashMap>`s
created empty per scope, and every name resolution walks the `ancestors()` outer
chain from the current scope to root
([`scope.rs`](../../src/machine/core/scope.rs) `ancestors` / `resolve_with_chain`,
with type, operator, and dispatch lookups structured the same way). So reaching a
builtin — the most common names: operators, `PRINT`, dispatch primitives — costs
O(scope depth), and a fresh scope is created per user-function call
([`kfunction/invoke.rs`](../../src/machine/core/kfunction/invoke.rs)), so depth
grows with call and recursion nesting. Builtins stay reachable from any depth only
through a special carve-out: a `BindingIndex` of `idx == 0` is always visible past
any lexical-frame cutoff (`visible()`, `bindings.rs`). There is also no dedicated
"can't shadow a builtin" gate — the general same-scope `Rebind` error
(`bindings.rs` `try_apply`) only fires on a collision in the *local* map, which
for builtins is the root alone.

**Impact.**

- *Builtin lookups resolve without walking to root.* The hottest names reach a
  binding at the current scope, so resolution cost stops scaling with call and
  recursion depth.
- *Builtins become ordinary bindings, not a visibility special case.* Present in
  every scope, they no longer need the `idx == 0`-always-visible carve-out, and the
  no-shadow invariant for Name-keyed builtins is enforced by the same `Rebind`
  machinery everywhere rather than only at the root scope.
- *Function and operator builtins stay overloadable.* A user FN or operator over a
  builtin keyword keeps accumulating as a dispatch overload, now resolved against a
  locally-present bucket.

**Directions.**

- *Seed every scope with the builtins — decided.* Each scope exposes the builtin
  `types` / `functions` / `operators` entries, shared rather than copied so per-call
  scope creation stays cheap.
- *Realization — open; decide by benchmark.* Two shapes to prototype and measure
  against today's HashMap-plus-walk:
  - *Shared builtin layer.* Each scope keeps its mutable local HashMaps plus a
    pointer to one shared immutable builtin `Bindings`, consulted on a local miss.
    Keeps O(1) lookups and adds no dependency; the bind path gains an explicit
    consult of the layer so Name-keyed collisions still `Rebind`.
  - *Persistent maps in `Bindings`.* The maps become persistent/immutable; every
    scope is a shared builtin base plus a local overlay, so builtins are ordinary
    local entries and the `Rebind` / no-shadow behaviour needs no extra check. Costs
    a persistent-map dependency, changes all eight map types, and needs structural
    merge for the overload buckets.

  Benchmark the walk-elimination speedup against the persistent-map lookup cost
  before committing.
- *Bind semantics stay split by key kind — decided.* Name-keyed builtins (types)
  rebind-error on a user collision — the no-shadow win. Bucket-keyed builtins
  (`FN` / `FUNCTOR` / operators, via `BinderKey::Bucket`,
  [`submit.rs`](../../src/machine/execute/scheduler/submit.rs)) **merge** — a user
  overload accumulates into the seeded bucket and dispatch picks by specificity, so
  seeding a bucket must preserve the builtin overloads, never replace them.
- *Drop the builtin visibility carve-out — open.* With builtins present in every
  scope, the `idx == 0`-always-visible rule is redundant; confirm nothing else
  relies on it (notably its interaction with the `nominal_binder` cutoff carve-out)
  before removing.

## Dependencies

**Requires:** none — engine-internal.

**Unblocks:** none tracked yet.

This reshapes the scope-lookup layer described in
[design/typing/lookup-protocol.md](../../design/typing/lookup-protocol.md); update
that doc when it lands. A sibling of the other refactor-hygiene items, independent
of them.
