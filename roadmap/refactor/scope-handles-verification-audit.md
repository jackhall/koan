# Rebuild the scope-handles verification list

Re-anchor `design/per-call-region/scope-handles.md § Verification` on tests that
still exist.

**Problem.** The `## Verification` section of
[scope-handles.md](../../design/per-call-region/scope-handles.md) cites four
tests that no longer exist in the suite. Three named the retired TCO
frame-reuse mechanism —
`call_arena_try_reset_for_tail_round_trip`,
`call_arena_try_reset_for_tail_refuses_when_aliased`, and
`call_arena_try_reset_for_tail_allows_reset_under_escaped_storage` — and were
deleted with `try_reset_for_tail` (the current TCO path reinstalls the slot and
turns over the region, [tail-call-optimization.md](../../design/tail-call-optimization.md);
no in-place reset remains). The fourth,
`alloc_object_redirects_self_anchored_value_to_escape_arena`, is an unrelated
cycle-gate test that is also gone. The section therefore points a reader at four
nonexistent tests as the verification anchor for a still-live protocol.

**Acceptance criteria.**

- Every test named in `scope-handles.md § Verification` resolves to a test that
  exists in the suite.
- The self-anchored-value escape-redirection behavior (formerly pinned by
  `alloc_object_redirects_self_anchored_value_to_escape_arena`) is either cited
  by its current test or the bullet is removed if the behavior is no longer
  distinct.
- The tail-region uniqueness / escape invariant is cited by a current test
  (`chained_tail_calls_reuse_frames`, `chained_user_fn_tail_calls_reuse_one_slot`,
  `match_driven_tail_recursion_completes`, `recursive_tagged_match_no_uaf`, or
  their successors), described in terms of the current reinstall/region-turnover
  model, not the retired reset mechanism.

**Directions.**

- *Scope — decided.* Doc-only: reconcile the one section against the current
  suite; no code change. Delete the section outright if its coverage is fully
  subsumed by the surviving TCO arena tests.
- *Test discovery — open.* Whether to map each dead bullet to a current
  equivalent or fold the section into a shorter cross-link to the TCO arena
  tests. Recommended: locate the current tests first (`src/builtins/fn_def/tests/arena.rs`
  and neighbours), then decide per bullet.

## Dependencies

**Requires:** none — a leaf doc audit.

**Unblocks:** none.
