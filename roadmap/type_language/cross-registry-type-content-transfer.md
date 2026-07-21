# Cross-registry type-content transfer

**Problem.** The type registry is per-run-frame: every type's content is an interned
[`TypeNode`](../../src/machine/model/types/node.rs) owned by the run frame's
[`registry.rs`](../../src/machine/model/types/registry.rs), and a `KType` is only a `Copy`
digest handle into that one registry. A handle carries no content, so it derefs only in
the frame whose registry interned it. Nothing moves type content from one frame's
registry into another's, which blocks two boundaries the language is otherwise ready to
cross:

- *Cross-run (sequential), reachable today.* A second `KoanRuntime` driven over one
  persistent scope mints its own registry, so the type handles the first run committed
  into the scope's `types` bindings name nodes the second run cannot deref: the second run
  panics in [`resolve_type_identifier`](../../src/machine/execute/dispatch/resolve_type_identifier.rs)
  ("type handle names no interned node") before any declaration logic runs. This is why the
  cross-run declaration-`Rebind` guarantee is pinned only at the install door
  (`cross_run_redeclare_rebinds_on_run_qualified_handle`) rather than end-to-end.
- *Cross-thread (concurrent), latent until concurrency ships.* Each future worker thread
  runs under its own run frame and owns its own registry
  ([design/typing/type-registry.md § Concurrency](../../design/typing/type-registry.md)),
  so moving a value between threads needs its types' content in the receiving frame's
  registry before the value's handles mean anything there.

Both faces are one gap: type content does not cross a registry boundary. Digests agree
across registries by content, so the handles themselves need no translation once the
content they name is present on both sides.

**Acceptance criteria.**

- A value carried into a run frame other than the one that interned its type resolves its
  `KType` handles through the receiving frame's registry — the content its digests name is
  present there, minted locally or transferred, never a dangling handle.
- A second `KoanRuntime` driven over a persistent scope resolves the types an earlier run
  installed into that scope, with no `resolve_type_identifier` panic, and a cross-run
  redeclaration of one name raises `Rebind` through the full submission pipeline — pinned
  by an end-to-end test that supersedes the door-level
  `cross_run_redeclare_rebinds_on_run_qualified_handle`.

**Directions.**

- *Handles need no translation — decided.* A digest is the same value in every registry,
  so transfer moves content, never rewrites handles.
- *Transfer mechanism — open.* Two candidates from
  [design/typing/type-registry.md](../../design/typing/type-registry.md): copy the value's
  type nodes plus everything reachable through their composition edges, skipping any digest
  the receiver already holds; or, with persistent (immutable) node storage, merge the two
  maps outright and share structure instead of copying.
- *Whether subtype-verdict edges transfer as warm cache — open.* A transferred type's
  memoized match verdicts could ride along or be recomputed on the receiving side; the two
  candidate mechanisms answer it differently.

## Dependencies

The cross-thread face is exercisable only once concurrency primitives exist; the cross-run
face is exercisable today, so this item is not gated on concurrency.

**Requires:** none — the substrate (per-frame registry, content-addressed digests) is
shipped.

**Unblocks:** none.
