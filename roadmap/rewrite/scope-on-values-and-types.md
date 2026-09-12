# Scope on values and types

The rewrite's top layer: lexical scopes, binding tables and dispatch, built
over the values module and the type lattice.

**Problem.** The old runtime's scope is
[machine/core/bindings.rs](../../src/machine/core/bindings.rs): two `RefCell`
channels (values, and the keyed channel of types, functions and operators)
behind a `WriteGate` capability, a claim store for still-finalizing binders,
positional visibility over statement indices, and the dispatch buckets the
keyworded builtins register into. It grew by accretion around the old
scheduler's step order — the claim store exists because a binder's finalize is
a separate step from its bind, the `RefCell`s because a step could hold two
borrows — and its lookup semantics are duplicated by the inferred-`CLOSE` walk
([free-identifier-walk-sourcing.md](../old_refactor/free-identifier-walk-sourcing.md)).
The type lattice ([type_lattice.rs](../../src/type_lattice.rs)) is a closed
algebra that reaches no scope type, and `memory`'s `ScopeId` is an identity
source; the scope layer that keys off both does not exist outside the old
runtime.

**Acceptance criteria.**

- A `scope` module depends on `values`, `type_lattice`, `memory` and `parse`,
  and on no scheduler type; the scheduler reaches scopes through the embedder
  contract, never the reverse.
- A scope's binding tables are bump-backed in the frame's region, and a scope
  is `Drop`-free.
- One visibility predicate over `(declared position, reader position)` answers
  every lookup, and the inferred-capture walk calls the same predicate.
- The value channel and the type channel are a structural partition enforced
  by the key types, and a name that classifies in neither is rejected where
  its text is classified.
- Dispatch keys a keyworded registration on its full bucket key, and a typed
  argument that does not satisfy a candidate is a non-match that falls through
  rather than a bind-time error.
- A binding resolves by name through the scope walk at its use site; no
  binding stores a resolved reference into another scope.
- The old runtime's tutorial programs run on the rewritten stack and print the
  same output, and `tools/verify_snippets.py` reads the rewritten binary.
- `src/scope/README.md` is the module's design doc, written fresh rather than
  migrated from `old_design/`, and the module's top-of-file comment links it
  ([design-docs-in-modules.md](design-docs-in-modules.md)).

**Directions.**

- *Whether a claim store survives — open.* If a binder's bind and finalize are
  one step under the new scheduler, a binding is committed or absent and the
  parked state disappears; if they stay separate, the three-state cell the old
  value channel used is the smaller of the two mechanisms. Recommended: make
  them one step and delete the store.
- *Interior mutability — open.* `RefCell` per channel, a single write door the
  scheduler hands the step, or scopes as values the step rebuilds. Recommended:
  the write door — it is what `WriteGate` was reaching for, without the
  runtime borrow flag.
- *Type-lattice API — decided.* The lattice is adapted to, not changed: a
  scope calls `is_subtype_of`, `satisfied_by`, `admits_with` and the
  registry's intern verbs as they stand
  ([old_design/typing/](../../old_design/typing/README.md)).

## Dependencies

**Requires:**

- [Values on memory](values-on-memory.md) — a binding table holds values and their carried types.
- [Scheduler on cellgraph](scheduler-on-cellgraph.md) — running a program through a scope needs the scheduler that drives it.

**Unblocks:** none tracked yet — the layer the builtins are re-seeded onto.
