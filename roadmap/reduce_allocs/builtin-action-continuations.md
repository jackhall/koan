# Builtin action continuations

**Problem.** Builtin bodies hand the engine their wake logic through the `Action`
currency's boxed closures
([`AwaitContinue` / `CatchContinue`](../../src/machine/core/kfunction/action.rs)), which
take the Boxed tier of
[design/execution/continuations.md](../../design/execution/continuations.md). A steady
tail-loop step routes MATCH through that surface, so it still buys one or two heap
boxes per step after the engine's own continuations are bumped.

**Directions.**

- *Currency — open.* Either the `Action` finish currencies are retyped so a `Copy`
  builtin finish reaches the bumped tier through the same single-erasure door, or the
  hot builtins (MATCH first) get `Copy` finish closures under the existing currency.
  Either way the builtin surface stays an open composition surface — no central
  enumeration of builtin wake logic.

**Acceptance criteria.**

- A steady-state MATCH-carrying tail-loop step allocates no Boxed continuation: a
  re-profile of `audit/shapes/tail_loop_steps100.koan` attributes no per-step
  continuation-box term to the `Action` surface, the `step` term in
  [`observe/alloc/terms.txt`](../../observe/alloc/terms.txt) drops by that share, and
  the tail-loop bound in `tests/allocation_baseline.rs` is re-measured.

## Dependencies

**Requires:**


**Unblocks:** none.
