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

**Acceptance criteria.**

- The hottest names (operators, `PRINT`, dispatch primitives) resolve in constant
  time via a scope's direct reference to the immutable root, rather than O(scope
  depth) up the `ancestors()` walk.
- Builtins live once in the immutable root and resolve from any scope through that
  direct reference — not copied into each scope — and the `idx == 0`-always-visible
  carve-out in [`bindings.rs`](../../src/machine/core/bindings.rs) is gone.
- A user binding that collides with a Name-keyed builtin (a type) raises the
  same `Rebind` error at any scope depth, via a bind-path consult of the root
  rather than a root-local collision only.
- A user FN or operator over a builtin keyword accumulates as a dispatch
  overload resolved against the root's builtin bucket, and dispatch still picks
  the builtin overload by specificity when no user overload matches.

**Directions.**

- *Seed every scope with the builtins — decided.* Each scope exposes the builtin
  `types` / `functions` / `operators` entries, shared rather than copied so per-call
  scope creation stays cheap.
- *Immutable, distinctly-typed root — decided.* The run-global root holds builtins
  only and accepts no user bindings — it is immutable, and a distinct type from the
  mutable scopes below it. The distinct typing is load-bearing beyond lookup: it makes
  the root genuinely run-lived, so a frame-bounded scope can still reach a run-lived
  root by reference (the enabler the frame re-anchor consumes — see Unblocks).
- *Insert a `RunScope` for top-level binds — decided.* A new mutable scope between the
  immutable root and user code receives top-level Koan bindings, so the root itself
  stays binding-free.
- *Leaf-aware root consult — decided.* A resolve consults the shared root once, via the
  direct reference, rather than re-checking it at each layer of the `ancestors()` walk;
  scopes track whether they are the resolution leaf so only the leaf does the consult.
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
  relies on it before removing.

## Dependencies

A sibling of the other refactor-hygiene items, independent of them; reshapes the
scope-lookup layer
([design/typing/lookup-protocol.md](../../design/typing/lookup-protocol.md)), so
update that doc when it lands.

**Requires:** none — engine-internal.

**Unblocks:**

- [Type-enforced frame re-anchor](type-enforced-frame-reanchor.md) — the immutable, distinctly-typed
  root gives a frame-bounded `&'s` scope a reachable run-lived `&'a` anchor, dissolving the
  Root-storage bind that blocks the `'s` split.
