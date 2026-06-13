# Unify the scheduler interface around a three-way node outcome

Collapse every scheduler-facing surface — dispatch, invoke, builtins, finishes — onto one
read-only view in, one three-variant outcome out, with the harness as sole graph writer.

**Problem.**

The scheduler is reached through four overlapping surfaces with no single interface: the
[`SchedulerHandle`](../../src/machine/core/kfunction/scheduler_handle.rs) trait (`&mut dyn`,
read and write methods mixed), whose sole implementor is
[`Scheduler`](../../src/machine/execute/scheduler.rs); the read-only
[`DispatchCx`](../../src/machine/execute/dispatch/ctx.rs), a hand-maintained mirror of the
scheduler's read methods; raw `&mut Scheduler` calls in the harness (`add_park_edge`,
`add_owned_edge`, `free`, `clear_dep_edges`, `schedule_*_literal`); and `BodyCtx` for builtins.

Two finish conventions coexist: host-side `CombineFinish` / `CatchFinish` take `&mut dyn
SchedulerHandle` and may write the graph mid-finish, while
[`DispatchCombineFinish`](../../src/machine/execute/nodes.rs) takes a read-only `&DispatchCx`
and returns a `DispatchOutcome` the harness applies. The `&mut dyn SchedulerHandle` survives in
exactly two closures — the `run_action` wrappers (`harness.rs:83`, `harness.rs:95`) — and only
because [`run_action`](../../src/machine/execute/harness.rs) *applies* a builtin's next `Action`
rather than returning it as data.

Separately, multi-statement FN bodies dispatch their non-tail (leading) statements as
fire-and-forget siblings: [`invoke`](../../src/machine/execute/dispatch/exec.rs) discards the
`dispatch_body_statements` ids (`exec.rs:97`), `finalize` never frees them (`execute.rs:177`),
and `free` cascades only through owned edges (`execute.rs:207`). A value-discarded
side-effecting leading statement (e.g. `PRINT`) is therefore neither sequenced before the tail
nor before the next iteration, and its un-reclaimed slot pins an `Rc` to its frame cart, so
`try_reset_for_tail`'s uniqueness gate (`arena.rs:530`) fails and `acquire_tail_frame` allocates
fresh — O(n) frames. Tail-call optimization is not flat for side-effecting multi-statement
bodies. (Data-dependent leading statements are saved by normal dispatch parking.)

**Acceptance criteria.**

- Every node step produces one of three outcomes: `Done` (a value to lift, or an error),
  `Continue` (replace this slot's work and frame, re-run, no park), or `ParkThenContinue` (park
  on deps; on resolve run a finish that yields another outcome).
- A Lift is a `Done` whose value is forwarded from one producer; it is the only park-and-forward
  shape and runs no finish.
- Every producer and finish has the shape `read-view → outcome`; none takes `&mut Scheduler` or
  `&mut dyn SchedulerHandle`. The read view permits scope binding (interior-mutable `&Scope`)
  but no graph writes.
- The harness is the only code that mutates the scheduler graph; the scheduler's
  edge/free/submit/frame primitives are private to it.
- `SchedulerHandle` does not exist as a trait; no single-implementor trait wraps `Scheduler`.
- A multi-statement FN body's leading statements are owned deps the activation parks on; they
  complete before the tail proceeds and cascade-free before the frame is reused, so
  tail-recursion with side-effecting statements runs in constant frame space.
- The scheduler cannot observe whether a step is dispatch, invoke, a builtin, or a re-run.
- `cargo test` and the Miri slate stay green.

**Directions.**

- *Three outcomes, not two — decided.* `Continue` carries new work + a `FramePlacement`;
  `ParkThenContinue` carries deps + a finish closure. The two continuation kinds don't unify (a
  finish returning `Continue` is circular), so the shape is irreducibly three-way.
- *`run_action` and `invoke` return outcomes, not effects — decided.* Both apply via `&mut`
  today; the deps they submit are already declarative (`Dep` / `DepPlacement`, `action.rs:234`).
  Converting them to `read-view → outcome` is the same change that retires the trait — the two
  `&mut`-using finishes (the `run_action` wrappers) become read-only once `run_action` returns
  its lowered outcome instead of recursing to apply it. A body-running producer takes a
  harness-provisioned frame as a resource; frame provisioning stays a harness concern via
  `FramePlacement`.
- *Leading statements become owned deps — decided.* The fire-and-forget TCO fix folds into this
  refactor rather than a separate item: preserving fire-and-forget would require a transitional
  outcome the unified model deletes anyway. `Action::Tail` / invoke with non-empty leading
  lowers to `ParkThenContinue { owned deps = leading, finish = Continue(tail) }`.
- *`DispatchState` dissolves into a resume continuation — decided.* Parked-dispatch resume
  becomes a `read-view → outcome` closure (as `DispatchCombineFinish` already is), so the
  scheduler never switches on dispatch-internal state. Birth is the same shape:
  `decide(read-view, expr) → outcome`.
- *`DispatchCx` becomes the one read view — decided.* Promote the hand-maintained mirror to the
  single `SchedulerView` handed to every decide and finish; no read/write trait split is needed since
  no caller needs polymorphism.
- *Lift kept as the push/notify single-producer primitive — decided.* In the taxonomy it is a
  deferred `Done`; in the implementation it stays the 1→1 forward node (`stamp_lift_ready`). Its
  `DeferTo` (spawn-child-and-lift) source is removed — a parking body returns `ParkThenContinue`
  applied to its own slot.
- *A single `Outcome` enum — decided.* `BodyResult` and `DispatchOutcome` collapse into one
  three-variant `Outcome`. Every variant of both maps onto `Done` / `Continue` /
  `ParkThenContinue` (see Outcome mapping), so after the refactor neither enum has a residual
  variant the other lacks — two enums would be structural duplicates, and a harness branching on
  two currencies would not be the single interface this item exists to build. The merge is a
  staged migration, not a design fork.

**Outcome mapping.** Every current step result maps as:

*Loop currency (`NodeStep`):* `Done(output)` → `Done`; `Replace{frame:Some, Dispatch}` (TCO) →
`Continue`(`ReuseReserve`); `Replace{frame:None, Dispatch}` → `Continue`(`Inherit`);
`Replace{Combine|DispatchCombine|Catch}` → `ParkThenContinue`; `Replace{Lift}` → `Done` (forward).

*Dispatch (`DispatchOutcome`):* `Terminal` → `Done`; `Combine` → `ParkThenContinue`;
`ParkSelf{state}` → `ParkThenContinue` (resume continuation replaces `DispatchState`);
`Redispatch` → `Continue`; `BecomeDispatch` → `Continue`; `ParkLift` → `Done` (deferred forward —
the surviving Lift); `Invoke` → run `invoke(read-view, frame) → outcome` (no longer a variant);
`ElaborateRecordType` → run `elaborate(read-view, frame) → outcome`.

*Callable result (`BodyResult`, builtin ≡ user):* `Value` / `Err` → `Done`; `Tail` (empty
leading) → `Continue`; `Tail` (non-empty leading) → `ParkThenContinue{owned=leading,
finish=Continue(tail)}`; `DeferTo` → `ParkThenContinue` on the slot directly (Role-B Lift removed).

*Builtin (`Action`):* `Done` → `Done`; `Tail` → as `BodyResult::Tail`; `Combine` →
`ParkThenContinue`(`ShortCircuit` on dep-error); `Catch` → `ParkThenContinue`(`RunFinish`).

*Finishes / producers (convention):* `CombineFinish` / `CatchFinish` (`&mut dyn SchedulerHandle
→ BodyResult`) → `read-view → outcome`; `DispatchCombineFinish` (`&DispatchCx → DispatchOutcome`)
→ unchanged (already the target); `Cont` / `CatchCont` (`&FinishCtx → Action`) → unchanged,
`Action` lowered by the now-pure `run_action`; `run_action` (`&mut, Action → BodyResult`) →
`read-view, Action → outcome`; `invoke` / `elaborate_record_value` → `read-view, frame, … →
outcome`; dispatch decide handlers → unchanged.

*Scheduler write primitives* (`add_park_edge`, `add_owned_edge`, `free`, `clear_dep_edges`,
`schedule_*_literal`, `add_dispatch_*`, `add_combine_*`, `add_catch_*`, `acquire_tail_frame`,
`with_active_frame`, `dispatch_body_statements`, `enter_block`, `enter_body_block`) → all private
to the harness; no trait surface.

## Dependencies

Follow-up to the shipped dispatch write-effect contract (the Invoke/Builtins/dispatch
scheduler-extraction arc), which made `Scheduler` the sole `SchedulerHandle` implementor and
introduced the read-only `DispatchCx` / `DispatchOutcome` split this generalizes. Subsumes the
retired `retire-scheduler-handle-trait` item.

**Requires:** none — the sole-implementor and read/decide/apply preconditions have shipped.

**Unblocks:** none.
