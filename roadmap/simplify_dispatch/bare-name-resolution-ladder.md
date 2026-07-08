# One bare-name-to-carrier resolution ladder

Own the dispatch-side bare-name → sealed-carrier ladder in one function whose
result type carries only the states its consumers can observe.

**Problem.** The scope-side lookups are unified and total, but their
dispatch-side consumers duplicate the ladder that turns a bare name into a
sealed delivered carrier. The full value → `resolve_value_carrier` /
type → `resolve_type_identifier` → seal / parked → park / unbound ladder is
written out twice — the wrap-slot arm of `part_walk`
([keyworded.rs](../../src/machine/execute/dispatch/keyworded.rs)) and
`resolve_aggregate_bare_name`
([literal.rs](../../src/machine/execute/dispatch/literal.rs)) — with the same
seal calls, differing only in unbound-fallback policy (error vs. a
sub-`Dispatch`). Fragments of the same ladder recur in `bare_type_leaf`
([single_poll.rs](../../src/machine/execute/dispatch/single_poll.rs)) and
`resolve_name_part` ([dispatch.rs](../../src/machine/execute/dispatch.rs)),
each with its own `TypeResolution` mapping and first-producer logic.

The result types force per-site dead arms. `NameOutcome` bundles `Cycle` and
`ProducerErrored`, which `keyworded::initial` short-circuits upfront, so
`part_walk` writes `unreachable!()` arms for them and literal.rs collapses the
same states into a fallback. `producer_disposition`'s four-arm ladder is
re-matched at five sites (dispatch.rs, keyworded.rs, fn_value.rs,
single_poll.rs ×2), each marking a different subset unreachable with a
justifying essay — the return type does not match how it is consumed.

**Acceptance criteria.**

- One function owns the bare-name → sealed-carrier ladder; `part_walk`'s
  wrap-slot arm and `resolve_aggregate_bare_name` both delegate to it, and the
  unbound-fallback difference is expressed as each caller's handling of an
  unbound result, not as a second copy of the ladder.
- The ladder's result type has exactly the states every consumer can observe
  (sealed, parked, unbound); producer-error and cycle states are absorbed once
  at the resolution surface, and no consumer of the ladder carries an
  `unreachable!()` arm for a pre-excluded state.
- `bare_type_leaf` and `resolve_name_part` derive their type-channel mapping
  and first-producer logic from the same surface rather than re-deriving it.
- No `producer_disposition` caller re-matches the four-arm disposition with
  per-site `unreachable!()` arms — the states a call site can see are narrowed
  where they are produced.
- Resolution behavior is unchanged — the same parks, forwards, seals, and
  errors at the same sites — with existing tests green.

**Directions.**

- *Result shape — open.* (a) A dedicated three-state result (sealed / parked /
  unbound) returned by one `resolve_bare_carrier`, with `NameOutcome` demoted
  to (or absorbed into) the entry surface; (b) keep `NameOutcome` and
  parameterize the ladder by a fallback-policy closure. Recommended: (a) —
  the narrowed type is what deletes the dead arms.
- *Where producer errors and cycles are absorbed — open.* Keep the existing
  entry-point short-circuit in `keyworded::initial` and make the ladder's
  contract "already screened", or fold the screening into the ladder itself so
  every entry path gets it. Recommended: fold into the ladder — the contract
  stops being positional.
- *Head-callability fold-in — open.* `type_call`
  ([single_poll.rs](../../src/machine/execute/dispatch/single_poll.rs)) and
  `classify_head`
  ([head_deferred.rs](../../src/machine/execute/dispatch/head_deferred.rs))
  duplicate the "what is a callable type value" classification downstream of
  this ladder; decide whether unifying those two arms rides this item or
  stays a separate cleanup.

## Dependencies

**Requires:** none — self-contained dispatch cleanup.

**Unblocks:** none tracked yet.
