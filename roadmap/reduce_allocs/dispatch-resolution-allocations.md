# Dispatch resolution allocations

**Problem.** The dhat profile of `audit/shapes/tail_loop_steps100.koan` (2026-08-25,
differenced against the 10-step run) attributes ≈6 heap allocations per step to
dispatch-side resolution transients, all build-use-drop within one decide:
`classify_for_pick` (2/step) and `lazy_eager_indices` (1/step) in
[src/machine/core/kfunction/pick.rs](../../src/machine/core/kfunction/pick.rs) build
per-call classification lists on the heap; `relaxed_admits` (1/step,
[src/machine/execute/decide/resolve_dispatch.rs](../../src/machine/execute/decide/resolve_dispatch.rs))
collects the relaxed-admission candidate set; and `user_type_refs` (2/step,
[src/machine/execute/decide/resolve_type_identifier.rs](../../src/machine/execute/decide/resolve_type_identifier.rs))
builds the pending-claim source list for every type-annotation read, populated or not.

**Directions.**

- *Transient homing — open.* The decide-local lists move to the step scratch arena
  (`ctx.scratch()`), or the per-call classification is memoized where it is derivable
  from the signature alone (`classify_for_pick` / `lazy_eager_indices` may be a mint-time
  property of the overload rather than a per-call computation). Recommended: scratch for
  the decide-locals; check the memoization question for the pick lists first, since a
  mint-time answer deletes the per-call work outright rather than re-homing it.

**Acceptance criteria.**

- A steady tail-loop step allocates no heap container in the dispatch resolve path: a
  dhat re-profile of `audit/shapes/tail_loop_steps100.koan` attributes no per-step term
  to `classify_for_pick`, `lazy_eager_indices`, `relaxed_admits`, or `user_type_refs`.
- The `step` and `dispatch` terms in
  [`observe/alloc/terms.txt`](../../observe/alloc/terms.txt) drop by the removed share,
  and the affected bounds in `tests/allocation_baseline.rs` are re-measured.

## Dependencies

**Requires:** none.

**Unblocks:** none.
