# Scope on values and types

The rewrite's lookup layer: lexical scopes and name resolution, built over the
values module and the type lattice.

**Problem.** The old runtime's scope is
[machine/core/bindings.rs](../../src/machine/core/bindings.rs): two `RefCell`
channels (values, and the keyed channel of types, functions and operators)
behind a `WriteGate` capability, a claim store for still-finalizing binders,
and positional visibility over statement indices, with a function body's
cutoffs assembled from its call site's chain
([lexical_frame.rs](../../src/machine/core/lexical_frame.rs)). Every lookup
walks the scope chain by name at run time. It grew by accretion around the old
scheduler's step order — the claim store exists because a binder's finalize is
a separate step from its bind, the `RefCell`s because a step could hold two
borrows — and its lookup semantics are duplicated by the inferred-`CLOSE` walk
([free-identifier-walk-sourcing.md](../old_refactor/free-identifier-walk-sourcing.md)).
The type lattice ([type_lattice.rs](../../src/type_lattice.rs)) is a closed
algebra that reaches no scope type, and `memory`'s `ScopeId` is an identity
source; the scope layer that keys off both does not exist outside the old
runtime.

**Acceptance criteria.**

- A `scopes` module depends on `values`, `type_lattice`, `memory` and `parse`,
  and on no scheduler type.
- A body's shape is built once, in program storage, and resolves every name the
  body reads to a slot of its per-call bindings, a slot of its closure
  bindings, or a builtin-table index; reading a resolved name visits no
  enclosing scope.
- An activation is bump-backed in its frame's region, is `Drop`-free, holds no
  pointer into itself and no placeholder in the closure bindings it copies, and
  a copy of it is a verbatim byte copy.
- One visibility predicate over `(declared position, reader position)`,
  measured at the definition site, answers every resolution, and a body does
  not see a later sibling of its own definition.
- The value channel and the type channel are a structural partition enforced
  by the key types, and a name that classifies in neither is rejected where
  its text is classified.
- A read of a visible slot that is still a placeholder reports it pending, and
  building closure bindings over a pending slot refuses to copy it.
- A user binding colliding with a builtin name in either channel is rejected,
  and a builtin reference reads through the activation header's base pointer.
- A by-name resolution from an activation picks the same binding as the
  coordinate the shape resolved for that name, and no binding stores a
  reference into another scope.
- [`src/scopes/README.md`](../../src/scopes/README.md) describes the module as
  shipped, and the module's top-of-file comment links it.

**Directions.**

- *Data model — open.* The design fixes the tiers and what each holds, not the
  structures. `parse`'s `SlotLayout` already carries a body's value binders
  sorted by symbol with their positions, and `memory`'s `SlotArray` is a
  `Copy`, `Drop`-free run of write-once slots. Recommended: build the shape on
  `SlotLayout` with a type half added, and the activation's slots on
  `SlotArray`.
- *What a pending slot carries — open.* Binders are known statically, so the
  scope needs only "pending" and "bound"; whether the pending arm names an
  in-flight producer or a waiter chain is the scheduler's choice. Recommended:
  leave the pending payload to the embedder, as `SlotArray` does.
- *Interior mutability — decided.* A placeholder is rewritten in place, once;
  bindings are semantically immutable.
- *Definition-site cutoffs — decided, provisionally.* The old runtime's
  call-site cutoff is not carried; mutual recursion between functions moves to
  a definition window ([callable-values.md](callable-values.md)). The
  consequences are unexplored until functions exist.
- *Builtin references — decided.* A builtin name resolves to a table index,
  read through a base pointer in the activation header. Copying the builtins a
  body names into each activation would cost a slot per builtin per call,
  multiplied by recursion depth, to save one load of a small table every body
  reads.
- *`EVAL` resolution — decided.* `EVAL` resolves by a by-name walk outward from
  the scope it appears in.
- *Which scopes keep a parent link — open.* Only an `EVAL` walk needs one.
  Recommended: a shape containing `EVAL`, and every shape lexically enclosing
  it, keeps its defining scope; everything else resolves through coordinates
  alone.
- *Type-lattice API — decided.* The lattice is adapted to, not changed: a
  scope calls `is_subtype_of`, `satisfied_by`, `admits_with` and the
  registry's intern verbs as they stand
  ([old_design/typing/](../../old_design/typing/README.md)).
- *A REPL's top-level scope — deferred.* Its bindings are not all known when it
  is created; designed once there is a REPL to serve.

## Dependencies

**Requires:** none — [values](../../src/values/README.md) and the
[type lattice](../../src/type_lattice/README.md) ship.

**Unblocks:**

- [Callable values](callable-values.md) — a callable captures a scope.
