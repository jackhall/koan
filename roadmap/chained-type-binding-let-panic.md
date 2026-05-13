# Chained type-binding LETs panic the scheduler

**Problem.** A program that chains two type-binding LETs and then uses the
second in an FN signature panics in
[`node_store.rs`](../src/runtime/machine/execute/scheduler/node_store.rs)
with `"result must be ready by the time it's read"`. Concrete reproducer:

```
LET MyT = Number
LET MyL = (LIST_OF MyT)
FN (USE xs: MyL) -> Number = (1)
PRINT (USE [1 2 3])
```

The first PRINT result (`1`) emits before the panic, so the `MyL`-typed FN
dispatch succeeds in routing the call; the panic surfaces during the
post-execution read of a slot whose result was eagerly freed or never
finalized. The bug is independent of the eager-type-elaboration work that
landed the parens-wrapped FN-parameter sub-Dispatch and phase-5 cleanup —
removing the chain (`LET MyL = (LIST_OF Number)` directly, no intermediate
`MyT`) sidesteps it, as does inlining the type at the FN's parameter slot.
The interaction is between the LET-binding-Combine `MyL` opens (parking on
`MyT`'s elaboration) and the FN's signature-elaboration park on `MyL`.

**Impact.**

- *Scheduler reliability under composed type aliases.* Type-binding LETs
  compose as the user expects rather than panicking the host on a
  scheduler-internal contract violation. The pattern `LET Aa = ... ; LET
  Bb = (... Aa ...) ; FN (... : Bb) -> ...` becomes uniformly writable without
  surface-order workarounds.
- *Diagnostic confidence around the FN-signature Combine path.* Any
  remaining scheduler interactions between type-LET Combine chains and
  downstream parking become observable as structured errors rather than
  host panics, so the same panic doesn't mask other gaps.

**Directions.**

- *Reproducer-first triage — decided.* The first step is a narrow Rust-side
  test exercising the four-line program above and asserting it terminates
  without panic and prints `1` twice. Adding it under
  [`fn_def/tests/return_type.rs`](../src/runtime/builtins/fn_def/tests/return_type.rs)
  (alongside `fn_with_user_bound_return_type_works`) keeps the related
  test slate together.
- *Root cause — open.* Two leading hypotheses: (1) the LET-binding `Combine`
  for `MyL` is reclaimed before the FN's signature-Combine reads its result,
  since the FN's park edge into `MyL`'s producer is a `DepEdge::Notify` that
  `free()` doesn't traverse; (2) the `MyL` result is finalized but the read
  happens on a stale `NodeId` that points at a recycled slot. Both reduce to
  the dep-graph entry the FN's Combine installs against the LET's producer
  — bisecting via a printf-style trace of `add_park_edge` / `free` calls
  around the failing dispatch should distinguish them.
- *Fix shape — open.* If the producer is reclaimed too early, the `free()`
  walk needs to leave slots with live `notify_list` entries intact (a
  predicate the walk already has via `is_live`, suggesting the contract
  isn't being honored on one path). If the consumer is reading a stale
  NodeId, the FN-signature Combine's dep list needs to capture the producer
  by some stable identity that survives slot recycling. Decision deferred
  until the trace lands.

## Dependencies

**Requires:**

**Unblocks:**

No hard prerequisites and no roadmap items downstream. The bug is
pre-existing and independent of the eager-type-elaboration work that
surfaced it.
