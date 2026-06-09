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

- *Direct root reference, not per-scope copies — decided.* Each scope carries a direct
  handle to the one immutable root (`Scope::root`, `None` iff the scope is the root) and
  reaches the builtins through it in one hop, rather than copying or seeding the builtin
  entries into every scope's maps. Per-call scope creation stays cheap (one extra pointer).
- *Immutable, distinctly-typed root — decided.* The run-global root holds builtins
  only and accepts no user bindings — it is immutable, and a distinct type
  (`ScopeKind::Root`) from the mutable scopes below it. The distinct typing is
  load-bearing beyond lookup: it makes the root genuinely run-lived, so a frame-bounded
  scope can still reach a run-lived root by reference (the enabler the frame re-anchor
  consumes — see Unblocks).
- *Insert a `RunScope` for top-level binds — decided.* A new mutable scope between the
  immutable root and user code receives top-level Koan bindings, so the root itself
  stays binding-free.
- *No-shadow consult hits the root directly — decided.* The nominal-seal path
  (`register_type_upsert`) asks the root whether a name is a builtin type and `Rebind`s
  the collision; because the consult reads the single root rather than each layer of the
  `outer` chain, it is depth-agnostic — shadowing a builtin type is illegal at any depth.
- *Leaf-aware lookup routing — open.* Builtin *resolution* still walks `outer` to the
  root; route it through the direct reference instead, consulting the root once (only the
  resolution leaf consults it) rather than re-checking at each `ancestors()` layer.
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
