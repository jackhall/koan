# Carrier-only catch delivery

The catch channel's watched value delivers as its sealed carrier — the last
value-copy dep handoff retires, completing the terminal-channel-only delivery
of [design/scheduler-library.md](../../design/scheduler-library.md).

**Problem.** `CatchOk.value` / `catch_continuation` hand the watched value to
a `Catch` finish as a bare relocated copy — the one remaining
`DepTerminal::relocate` caller
([catch.rs](../../src/builtins/catch.rs)). Every other consumer reads its
deps un-relocated as carriers (value and reach as one unit, adopted at the
consumer's step brand), so the catch channel is the single site where a dep
still crosses as a pinless copy, and the relocation hook
(`relocate_carried`, re-exported through the
[lift.rs](../../src/machine/execute/lift.rs) shim) survives for it alone.

**Acceptance criteria.**

- A `Catch` finish receives the watched value as its sealed carrier and
  adopts or opens it at its own step brand; no relocated copy of the watched
  value is made.
- `DepTerminal::relocate` is deleted, and `relocate_carried` has no callers
  (the [lift.rs](../../src/machine/execute/lift.rs) re-export shim retires
  with it).
- The catch behavior tests pass unchanged, and the Miri slate stays at
  0 UB / 0 leaks.

**Directions.**

- Delivery shape — decided per
  [design/scheduler-library.md](../../design/scheduler-library.md): the catch
  channel follows the carrier-carrying pattern the dispatch finishes use.
- Whether the watched value is adopted into the catch scope (reach folded,
  value re-anchored) or only opened at the step brand for the `CatchOk` arm —
  open. Adoption matches the bind path; a brand-scoped open suffices if the
  value never outlives the finish.

## Dependencies

**Requires:**
- [Fold embedded dep reach at the finish surface](fold-embedded-dep-reach.md)
  — establishes the fold discipline and helpers this finish adopts.

**Unblocks:** none — the last delivery-channel conversion.
