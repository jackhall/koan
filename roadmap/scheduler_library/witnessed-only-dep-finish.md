# One dep-finish delivery currency

Fold the value-copy dep-finish stack into the witnessed one, so the scheduler
boundary has a single delivery shape — the terminal-with-carrier delivery that
[design/scheduler-library.md](../../design/scheduler-library.md) § The
consumer API assumes.

**Problem.** "Run a finish over resolved deps" exists as two parallel stacks in
`src/machine/execute/outcome.rs`: the `Continuation::Finish` /
`Continuation::FinishWitnessed` variant pair (:121-132), the `DepFinish`
(:143-150) vs `WitnessedDepFinish` (:250-256) closure aliases, and the paired
short-circuit combinators. The bare stack hands finishes **relocated values**
(`&[Carried]` plus a parallel carrier slice); the witnessed stack hands
**`DepTerminal`s** (un-relocated value + the dep's own sealed carrier). The
`DepTerminal` docs (:167-179) describe both deliveries side by side, and
`catch_continuation` (:294-297) is a third spelling of the same consumption.
Every semantic change to dep delivery lands in two-plus places, and the
value-copy shape has no equivalent in the target library surface.

**Acceptance criteria.**

- One NodeWork delivery loop exists: it hands the finish machinery
  `DepTerminal`s (value + sealed carrier). The value-copy shape
  (relocate each terminal into the consumer region, collect the carrier
  slice) is derived from it by exactly one adapter.
- `short_circuit` and `short_circuit_witnessed` no longer exist as separate
  loops (whatever the `Await` builder left of them collapses onto the single
  delivery).
- The catch delivery (`catch_continuation`) consumes the same `DepTerminal`
  currency rather than re-implementing relocation.
- The builtin-facing `AwaitContinue` signature
  (`src/machine/core/kfunction/action.rs:210`) is unchanged — builtins are
  not migrated by this item; they sit behind the adapter.
- Behavior unchanged; existing tests green.

**Directions.**

- *Builtin-facing signature unchanged — decided.* Migrating builtin finishes
  onto terminals/step-context construction is the witnessed-hoist tranche,
  not this item. This item only unifies the plumbing beneath them.
- *Adapter shape — open.* (a) keep `Continuation::Finish` as a variant whose
  apply-side wraps the witnessed delivery with a relocation adapter; (b)
  delete the variant and have the harness wrap legacy finishes at
  construction. Recommended: (a) — smaller blast radius, one owner for
  relocation either way.
- *`CatchOk` construction — open.* Derive it from a `DepTerminal` helper vs
  leave as a specialized consumer of the unified loop. Either satisfies the
  criteria.

## Dependencies

**Requires:**


**Unblocks:** none tracked yet — the witnessed-hoist tranche (step
construction context) is planned as this ships.
