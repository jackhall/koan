# Fold the two type-write paths

**Problem.** `src/machine/core/bindings.rs` carries two near-identical type writers:
`try_apply_type` (lines 812-844, strict insert-if-absent) and
`try_register_type_upsert` (681-719, overwrite on a `PartialEq`-equal existing entry —
the seal pre-install rewrite). Both run the same skeleton — borrow `types`, cross-kind
probe of `data`, insert, drop the borrow, best-effort placeholder clear — and the
cross-kind exclusion block ("probe `data`, `Rebind` on hit, `Conflict` on borrow
failure") is verbatim in both (694-703 vs 830-839). The two diverge only at the
existing-entry policy, so a change to the shared skeleton (borrow order, exclusion
rule, placeholder clearing) must be made twice.

**Acceptance criteria.**

- One type-write path, parameterized by existing-entry policy; the cross-kind
  exclusion block appears once.
- `try_register_type` (strict `Rebind` on any existing entry) and
  `try_register_type_upsert` (equal-overwrite, `Rebind` on a non-equal entry) keep
  their public signatures and semantics, exercised by existing tests.

**Directions.**

- *Policy parameter shape — open.* (a) A two-variant policy enum
  (`Insert` / `UpsertEqual`) matched at the existing-entry probe; (b) a closure
  deciding the existing-entry outcome. Recommended: (a) — both policies are known and
  their rationale docs stay attached to named variants.

## Dependencies

Doing this first shrinks
[Unify the three deferred-write channels](unify-deferred-write-channels.md) — the
channel fold then wraps one type writer — soft ordering, not a prerequisite.

**Requires:** none — leaf cleanup.

**Unblocks:** none tracked.
