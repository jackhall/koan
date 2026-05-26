# Branch-arm return-type agreement

MATCH and TRY arms have no static return-type discipline. The builtins
declare `KType::Any`; arms can produce arbitrarily different shapes.

**Problem.** MATCH and TRY (`src/builtins/match_case.rs`,
`src/builtins/try_with.rs`) both register with `sig(KType::Any, ...)`,
so the `Resolved`-return check in `Scheduler::execute` is a no-op for
their values. Two arms can return different shapes; a downstream
consumer's success or failure depends on which arm took at runtime. The
divergent-result hazard mirrors the divergent-bind hazard already closed
structurally by the [lexical-provenance chain](../design/execution-model.md#lexical-provenance-chain)
making each arm its own block — the bind side is closed lexically, but
the result side is open.

**Impact.**

- *MATCH and TRY expressions carry a static return type.* A consumer
  can rely on the value's shape without inspecting which arm ran.
- *Dispatch on a MATCH / TRY value is statically admissible.* The
  receiving slot can match against the declared type at bind time
  rather than failing at one arm's runtime shape.
- *The arm-divergence error is structural and binder-local.* Surfaces
  at MATCH / TRY definition, not at the unrelated downstream consumer.

**Directions.**

- *Agreement vs union — open.* Three alternatives:
  - Require all arms to agree on a return type. Symmetric with FN
    parameter typing; uses the same machinery.
  - Synthesize a union across arms. The expression's type is the
    disjunction; composes with tagged unions when arm types are already
    nominal.
  - Hybrid: require agreement unless an explicit union annotation
    widens. Matches the FN return-type story (declared types coarsen
    inferred shapes).

- *Surface for declared return type — open.* Whether MATCH / TRY grow
  an annotation slot (`MATCH x :Number | ...`) or always infer from
  arms.

## Dependencies

**Requires:** none yet — design choice precedes wiring.

**Unblocks:** none tracked yet. Downstream consumers that dispatch on
MATCH / TRY values benefit, but no specific roadmap item is gated on
this today.
