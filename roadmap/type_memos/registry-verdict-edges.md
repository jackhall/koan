# Verdict edges on a run-frame type registry

Move subtype-verdict memoization out of thread-local state and onto a registry the
run frame owns. First of the four items that land
[design/typing/type-registry.md](../../design/typing/type-registry.md); this one
ships the registry substrate and its verdict edges, before any content moves in.

**Problem.** Subtype verdicts memoize in a `thread_local!` LRU
(`type_memos.rs`) — state homed
outside the run frame, and the one live counterexample of global runtime state in
the type layer. The predicates that consult it — `is_more_specific_than`
([`ktype_predicates.rs`](../../src/machine/model/types/ktype_predicates.rs)),
`SigSource::satisfied_by_module`
([`ktype.rs`](../../src/machine/model/types/ktype.rs)), and
`Module::structurally_satisfies`
([`module.rs`](../../src/machine/model/values/module.rs)) — are pure `&self`
methods with no context parameter, so nothing above them can own the cache, and the
LRU needs an eviction cap (65,536 entries) only because the thread-local outlives
every run.

**Acceptance criteria.**

- A `TypeRegistry` owned by the run frame records subtype verdicts as
  `(subject digest, candidate digest, relation) → bool` entries — negative verdicts
  recorded the same as positive — and drops with its run frame; verdict storage
  carries no eviction cap.
- The memoized predicates and the structural walks between them take a
  `&TypeRegistry` parameter threaded from the execution context; no code under
  `src/machine/model/types/` reaches `thread_local!` or `static` mutable state.
- `type_memos.rs` is deleted; `Relation` and the pre-seal digest guard (today's
  `memo_safe`, which keeps pointer-transient digests out of the memo) live on the
  registry, and the guard still gates verdict recording.
- The memo-hit assertions in the ascribe test suites read counters on the run's
  registry rather than thread-local counters.

**Directions.**

- *Home and reach — decided per [type-registry.md](../../design/typing/type-registry.md).*
  The registry is a component of the run frame
  ([`frame.rs`](../../src/machine/core/arena/frame.rs)), created where the run frame
  is adopted and handed to dispatch code as a `SchedulerView` field with an
  infallible accessor — the same shape as the view's `dest_frame` field.
- *Interior mutability — decided.* The context hands out `&TypeRegistry`, so the
  verdict map sits behind a `RefCell`; borrows stay short and never span re-entrant
  registry calls.
- *Content storage — deferred.* This item ships verdict edges only; type content
  moves onto the same registry through
  [Interned type content behind Copy handles](interned-type-content.md).

## Dependencies

**Requires:** none — the content digests the verdict keys use are shipped.

**Unblocks:**

- [Interned type content behind Copy handles](interned-type-content.md) — reuses
  the registry home and the `&TypeRegistry` threading through the predicate chain.
