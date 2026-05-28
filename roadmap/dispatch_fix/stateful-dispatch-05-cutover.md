# Stateful dispatch — Step 5: cutover

Flip the `Scheduler::use_stateful_dispatch` toggle's default from
`false` to `true`. Production dispatch now runs on the stateful
driver; the legacy `run_dispatch` remains in place behind the
toggle until [step 6](stateful-dispatch-06-deletion.md) deletes it.

**Problem.** After step 4, the stateful driver runs every
`DispatchShape` variant end-to-end under toggle-on and full
`cargo test` is green on both drivers. But production still
runs on the legacy `run_dispatch`: the toggle defaults to `false`
so toggle-off (the production default) bypasses the new path.
Until the default flips, the per-wake redundancy — re-walking
parts, rebuilding `bare_outcomes`, re-running admission — keeps
costing the production hot path on every wake.

**Impact.**

- Production dispatch runs on the stateful driver: per-wake cost
  drops to one O(1) splice per fired edge plus track-completion
  continuations on track-zero. The shape classifier runs once per
  dispatch.
- Real-world workloads exercise the stateful path; performance
  and Miri leak-slate behavior become observable on the new
  driver under the full corpus, not just the test slate. Surfaces
  any allocation-pattern regression before
  [step 6](stateful-dispatch-06-deletion.md) deletes the fallback.
- The toggle remains available as an emergency rollback: setting
  `Scheduler::with_stateful_dispatch(false)` or
  `KOAN_STATEFUL_DISPATCH=0` reverts a single binary to the
  legacy path without code changes.

**Directions.**

- **Toggle default flip — decided.** Change the `Scheduler`
  constructor in
  [`src/machine/execute/scheduler.rs`](../../src/machine/execute/scheduler.rs)
  to initialize `use_stateful_dispatch: true`. Leave the field
  and its builder accessor in place; the production default is
  the only change.

- **Miri leak slate — decided.** Run the full Miri leak slate
  via the `miri` skill against toggle-on (now the default) to
  confirm no leaks on the new combined-park-as-state paths.
  Particular attention: the `pre_subs` carry-through during the
  `Initialized → Keyworded` transition, and the
  `recent_wakes` outer Vec retention pattern from
  [step 2](stateful-dispatch-02-recent-wakes.md). Update
  [`observe/miri_slate.md`](../../observe/miri_slate.md) per the
  documentation skill's slate-duration logging rule.

- **Performance smoke — decided.** Run any benchmark or
  larger-fixture run (e.g. the recursive-body / tail-call test
  files at scale) against toggle-on and confirm no measurable
  regression. The stateful path saves work per wake but adds the
  `recent_wakes` append per fire — net should be a win, but
  confirm rather than assume.

- **Rollback path — decided.** No code change for rollback;
  `Scheduler::with_stateful_dispatch(false)` or the env-var
  flip suffices. The legacy `run_dispatch` body and the
  combined-park rewrite paths remain intact through this step.

- **Acceptance criteria — decided.** Full `cargo test` green with
  the new default. Miri full-slate green. No measurable
  performance regression on the existing fixture corpus.

- **Risks — open.**
  - A workload pattern not exercised by tests could surface a
    behavioral divergence under toggle-on. The rollback path
    above is the mitigation; if it fires, the divergence
    becomes a fixed-in-step-4-or-earlier issue rather than a
    rollback.
  - A Miri leak surfaced only by the new path's allocation
    pattern (e.g. a `recent_wakes` inner Vec retention case)
    blocks this step; fix the leak in the relevant prior step,
    not here.

## Dependencies

**Requires:**

- [Stateful dispatch — Step 4: `Keyworded` variant](stateful-dispatch-04-keyworded-variant.md)

**Unblocks:**

- [Stateful dispatch — Step 6: deletion](stateful-dispatch-06-deletion.md)
