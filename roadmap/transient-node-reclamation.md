# Transient-node reclamation

**Problem.** TCO's slot reuse covers only the outermost user-fn frame.
[`Scheduler`](../src/execute/scheduler.rs)'s `nodes`/`results` vecs still grow per
iteration whenever a body-internal sub-expression spawns a sub-`Dispatch`/`Bind`.
Realistic recursion (the predicate computation in an `IF`-guarded base case, or a
recursive call's argument expressions) accumulates entries. The `frame_holding_slots`
sidecar added during the leak fix is one piece of the substrate, but full
transient-node reclamation — detecting that a `Bind`/`Aggregate` and all its
sub-`Dispatch`es are no longer reachable and reclaiming their vec slots — is unbuilt.

**Impact.** This gates true O(1) tail-recursive memory. Factorial, list walk, and
similar patterns run in O(n) scheduler memory until it lands. It's the load-bearing
remaining problem from the leak fix.

## Dependencies

**Requires:**
- [Generalize `Scope::out` into monadic side-effect capture](monadic-side-effects.md) —
  the next pass for monadic effects revisits the surrounding `BuiltinFn` signature, and
  folding reclamation into the same pass keeps the rewrite cheap.
