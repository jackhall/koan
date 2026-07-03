# Own singleton nominal-set construction

**Problem.** The pending-member singleton set build —
`NominalMember::pending(name, scope_id, kind)` →
`Rc::new(RecursiveSet::new(vec![member]))` — is hand-rolled at roughly seven sites:
`src/machine/model/types/recursive_set.rs:135-137` (the in-module build),
`src/machine/model/types/resolver.rs:216`, `src/machine/core/kerror.rs:220-227`
(`synthetic_singleton`, which additionally `fill`s a throwaway schema outside the
normal seal path — its own doc calls the identity throwaway),
`src/builtins/newtype_def.rs:47-49`, `src/builtins/union.rs:237-242`,
`src/builtins/result.rs:26-31`, and `src/builtins/ascribe.rs:68`. The "one pending
member, then fill/seal" invariant is re-asserted per site rather than owned by
`RecursiveSet`. Each site's surrounding context differs (pre-seal identity install,
synthetic unregistered carriers, prelude registration), so each needs individual
verification before folding — only the kerror and newtype_def sites have been read in
context so far.

**Acceptance criteria.**

- `RecursiveSet` exposes one singleton-pending constructor, and every listed site
  routes it.
- The synthetic `KError` carrier (`kerror.rs`) builds through the shared constructor,
  with its throwaway-schema fill explicit at the call site.
- Existing behavior unchanged — existing tests green.

**Directions.**

- *Constructor shape — open.* (a) Pending-only
  (`RecursiveSet::singleton_pending(name, scope_id, kind)`) with `fill` left to
  callers; (b) an additional filled variant for the synthetic-carrier case.
  Recommended: (a) — smallest surface; the kerror fill stays visible where its
  throwaway nature is documented.

## Dependencies

**Requires:** none — leaf cleanup.

**Unblocks:** none tracked.
