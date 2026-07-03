# `Action::Tail` covers every dispatch tail

Make the action harness's `Tail` lowering complete, so dispatch stops
hand-building tail envelopes — the "`Action` is complete over its lowering"
layer of [design/scheduler-library.md](../../design/scheduler-library.md).

**Problem.** `run_action`'s `Action::Tail` arm
(`src/machine/execute/runtime.rs:208-285`) cannot express a leading-carrying
tail with `FramePlacement::Inherit` + `BlockEntry::FrameScope` — the lowering
at runtime.rs:255-258 expects an overlay in that position. Because of that
gap, `invoke` (`src/machine/execute/dispatch/exec.rs`) hand-lowers its two
tail shapes (`ExecOutcome::Tail`, :151-207, and `ExecOutcome::DeferredExprTail`,
:208-269), re-implementing the same skeleton `run_action` owns: clone the
leading statements into one `BodyBlock` dep, box a finish emitting
`Continue { work: decide(tail), frame, contract, block_entry, body_index }`,
wrap in `ParkThenContinue { park_count: 0 }`. Three copies of the `body_index`
arithmetic (`leading.len() + 1`) must stay in sync, and the
`DeferredExprTail` arm additionally derives its contract from the last dep's
result inside the finish.

**Acceptance criteria.**

- The `FramePlacement` × `BlockEntry` matrix accepted by the `Tail` lowering
  includes the leading-carrying `Inherit` + `FrameScope` combination.
- `dispatch/exec.rs` contains no `BodyBlock` / `ParkThenContinue` / `Continue`
  assembly for tails: both tail shapes construct an `Action::Tail` (or call
  the one lowering), and the lowering lives in one place.
- The `body_index` arithmetic exists once, in the lowering.
- The deferred contract case (contract resolved from the last dep's result at
  finish time) is expressible through the single lowering.
- Tail-call behavior is unchanged: tail chains stay flat, contract keep-first
  at the slot unaffected; dispatch-shape and tail-call tests green.

**Directions.**

- *Extend the matrix, don't share a constructor — decided* per
  [design/scheduler-library.md](../../design/scheduler-library.md): dispatch
  hands tails to the harness's one lowering. (The superseded alternative — a
  constructor shared by three hand-lowerings — is parked in
  `scratch/fold-deferred-tail-scaffold-leftover.md`.)
- *Contract derivation — open.* (a) a contract-source enum on `Action::Tail`
  (`Eager(Option<ReturnContract>)` / `FromLastResult`); (b) the lowering takes
  a closure producing the `Continue` fields. Recommended: (a) — keeps the
  `Continue` assembly inside the lowering.
- *Entry point — open.* `invoke` returns an `Action` to the harness vs calls
  the lowering function directly from dispatch. Either satisfies the criteria;
  prefer whichever keeps the decide→outcome→apply boundary
  ([design/execution/scheduler.md](../../design/execution/scheduler.md)) intact.

## Dependencies

**Requires:**

- [The `Await` envelope builder](await-envelope-builder.md) — the single
  lowering assembles its envelope through the builder instead of adding one
  more hand-rolled site.

**Unblocks:** none tracked yet — the later scheduler-library tranches
(witnessed construction hoist, regions-wholesale ownership) are planned as
these ship.
