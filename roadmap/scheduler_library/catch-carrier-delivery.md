# Carrier-only catch delivery

The catch channel's watched value delivers as its sealed carrier — the last
value-copy dep delivery retires, completing the terminal-channel-only
delivery of [design/scheduler-library.md](../../design/scheduler-library.md).

**Problem.** `catch_continuation`
([outcome.rs](../../src/machine/execute/outcome.rs)) hands a `Catch` finish
the watched value as `CatchOk.value` — a bare relocated copy made via
`DepTerminal::relocate`
([action.rs](../../src/machine/core/kfunction/action.rs)), whose validity
rests on the step pin rather than a carrier naming its reach. Every other
dep delivery reads its terminals un-relocated as carriers (value and reach
as one unit, adopted at the consumer's step brand), so the catch channel is
the single delivery where a dep still crosses as a pinless copy — and its
consumers barely need it: [catch.rs](../../src/builtins/catch.rs) already
folds `CatchOk.carrier` and ignores the copy, while
[try_with.rs](../../src/builtins/try_with.rs) clones the copy once more into
its `it` bind. The structural-copy hook backing the channel
(`relocate_carried`) lives in `machine::core`
([arena.rs](../../src/machine/core/arena.rs)) solely so
`DepTerminal::relocate` can reach it, re-exported through the
[lift.rs](../../src/machine/execute/lift.rs) shim. Its two other callers are
not deliveries and stay: `KoanRuntime::relocate_terminal`
([runtime.rs](../../src/machine/execute/runtime.rs), the `Forward`-ready
pull / run-root drain) and `park_on_literal`
([single_poll.rs](../../src/machine/execute/dispatch/single_poll.rs), the
literal fold) run it as the copy callback *inside* witnessed folds
(`transfer_lifted` / `transfer_into`), where the call shape folds the
producer's reach into the result carrier — the sanctioned construction
pattern, guarantee 5 of the design doc.

**Acceptance criteria.**

- A `Catch` finish receives the watched value as its sealed carrier and
  adopts or opens it at its own step brand; `CatchOk` and its relocated
  `value` no longer exist, and no relocated copy of the watched value is
  made anywhere on the catch path.
- `DepTerminal::relocate` is deleted, and the structural-copy hook is
  callable only from the execute layer's witnessed folds: it leaves
  `machine::core`, the [lift.rs](../../src/machine/execute/lift.rs)
  re-export shim retires, and the hook's only callers are the two
  storage-bound folds (`relocate_terminal`, `park_on_literal`) — a pinless
  copy is inexpressible outside a witnessed fold.
- The catch behavior tests pass unchanged, and the Miri slate stays at
  0 UB / 0 leaks.

**Directions.**

- Delivery shape — decided per
  [design/scheduler-library.md](../../design/scheduler-library.md): the catch
  channel follows the carrier-carrying pattern the dispatch finishes use.
- Hook disposition — decided: the hook moves into
  [lift.rs](../../src/machine/execute/lift.rs) as that file's owned item
  (the shim becomes the owner), renamed `copy_carried` with
  `pub(in crate::machine::execute)` visibility and a doc stating its sole
  role as the fold callback; its behavior tests stay co-located at
  `lift/tests.rs`. Inlining the copy at the two fold sites was rejected —
  it duplicates the match and strands the tests without a subject.
- Whether the watched value is adopted into the catch scope (reach folded,
  value re-anchored) or only opened at the step brand for the success arm —
  open. Adoption matches the bind path; a brand-scoped open suffices if the
  value never outlives the finish. Recommended: `TRY-WITH`'s `it` outlives
  the finish (it rides the arm seed), so adopt at bind time — one clone made
  directly into the arm frame inside the carrier's open; `CATCH`'s fold
  needs no open at all.

## Dependencies

**Requires:** none — the fold discipline this finish adopts has shipped.

**Unblocks:** none — the last delivery-channel conversion.
