# Stateful dispatch — Step 4: `Keyworded` variant

Implement the rich `Keyworded` variant of `DispatchState`. This is
the variant the entire refactor is for: the one that today re-walks
every part, rebuilds the `bare_outcomes` cache, and re-runs strict
admission on every wake. After this step, `Keyworded` carries its
progress in the slot and advances by one edge per callback.

**Strategy: reimplement, don't reuse.** The stateful path is a
complete reimplementation of the keyworded dispatch behavior, not a
wrapper around `run_dispatch`. The stateful driver never delegates
to `run_dispatch`. Pure helpers that read state without mutating it
(`build_bare_outcomes`, `keyworded_part_walk`) are shared between
the two drivers; mutating helpers (`install_combined_park`,
`park_pending_and_redispatch`, `schedule_eager_only`, the `Bind`
slot construction) are replaced sub-step by sub-step with
state-bearing Track machinery on the stateful path. The legacy
mutating helpers stay alive only for the toggle-off `run_dispatch`
path; their last caller goes away in step 6.

Sub-step 4a has landed: the one-shot path (Resolved with no parks
and no eager subs) terminalizes directly on the stateful driver
via `Scheduler::stateful_keyworded_initial` in
[`run_dispatch_stateful`](../../src/machine/execute/scheduler/dispatch.rs).
The stateful entry runs every `ResolveOutcome` branch directly —
the `Deferred` / `ParkOnProducers` branches still invoke today's
`schedule_eager_only` / `park_pending_and_redispatch` /
`install_combined_park` mutating helpers as a transitional state
until 4b/4c/4d reimplement those code paths as Track installs on
`KeywordedState`. `KeywordedState` carries no fields yet; that
lands with the tracks.

**Problem.** After step 3, the five fast-lane `DispatchShape`
variants run on the stateful driver under toggle-on, but
`Keyworded` still delegates to the legacy `run_dispatch`. The
keyworded path is where the per-wake redundancy
(re-classifying the shape, rebuilding the `bare_outcomes` cache,
re-running strict admission, re-walking every part) actually hurts:
it's the variant with the `bare_outcomes` cache, strict admission,
post-walk wrap/ref-name/eager-sub splice, and the combined-park
rewrite. None of that machinery is yet state-bearing.

**Impact.**

- The `KeywordedState` struct carries `bare_outcomes`, the picked
  `&KFunction`, its `ClassifiedSlots`, the spliced
  `working_expr`, and three optional tracks
  (`bare_name_park: Option<Track>`, `eager_subs: Option<Track>`,
  `overload_park: Option<Track>`). Each track is an
  `(initial_count, on-zero continuation)` pair; the per-edge
  callback is O(1) (decrement counter, splice one slot, fire
  continuation only on track-completion).
- The shape classifier runs once per dispatch instead of once per
  wake. For a `Dispatch` that parks N times before terminalizing,
  that's an N→1 reduction. Strict admission re-attempts trigger
  only on track-completion, not per-edge.
- `install_combined_park` and
  `park_pending_and_redispatch`'s reason for existing — that
  re-dispatch on wake would re-stage already-submitted children —
  collapses for the new path. The slot remembers what it has
  scheduled; the legacy rewrite paths remain only for the still-
  in-place legacy driver until step 6.
- Composes with [SCC-aware dispatcher for parameterized self-
  recursive types](scc-aware-dispatcher-for-self-recursive-types.md):
  the SCC threading carrier slots naturally into `KeywordedState`
  as a new field, once that work picks up.

**Directions.**

- **Sub-step order — decided: increasing scope, never regressing.**
  Each sub-step lands as a separate PR or commit; toggle-on
  `cargo test` advances strictly green at each boundary.
  - **(4a) One-shot path (no parks). Shipped.** Implemented as
    `Scheduler::stateful_keyworded_initial` in
    [`dispatch.rs`](../../src/machine/execute/scheduler/dispatch.rs),
    routed from `run_dispatch_stateful`'s `Keyworded` arm. The
    handler runs each `ResolveOutcome` branch directly and
    terminalizes when no producer parked and no eager subs needed
    scheduling. `build_bare_outcomes` and `keyworded_part_walk`
    were factored out as pure helpers and are shared with
    `run_dispatch`. The Resolved-with-parks / Resolved-with-eager-subs
    sub-cases and the `Deferred` / `ParkOnProducers` branches still
    call the existing `install_combined_park` /
    `park_pending_and_redispatch` / `schedule_eager_only` mutating
    helpers — those calls are the transitional state 4b/4c/4d
    reimplement as Track installs on `KeywordedState`.
    `KeywordedState` carries no fields yet; that lands with the
    tracks.
  - **(4b) Eager-subs track + Deferred fold.** Replaces the
    stateful path's calls to `schedule_eager_only` and the
    inline `NodeWork::Bind { expr, subs }` construction. On
    wake, drain `recent_wakes`, splice each fired producer's
    terminal as `Future(obj)` into `working_expr.parts[i]`,
    decrement the track. On track completion, re-resolve dispatch
    against the spliced expression (folding in the `Deferred`
    branch with `function = None`) and call
    `function.bind(working_expr)` + `invoke_to_step` inline. No
    `Bind` slot allocation on the stateful path — the keyworded
    re-resolve replaces what today's `run_bind` does, eliminating
    the per-call Bind hop the legacy driver pays.
  - **(4c) Bare-name park track.** `bare_name_park: Some(Track
    { producers, splice_indices })`. Equivalent of today's
    `install_combined_park` folded into the variant's state.
    Cycle check via `DepGraph::would_create_cycle` runs at
    track-installation time, same surface today's fused walk
    uses. Re-admission continuation re-attempts strict
    admission against the now-bound types.
  - **(4d) Overload park track.** `overload_park: Some(Track {
    producer })` for the `ResolveOutcome::ParkOnProducers` arm
    that today fires when an innermost-visible
    `pending_overloads[key]` is recorded. Track-completion
    continuation re-runs
    [`resolve_dispatch_with_chain`](../../src/machine/core/scope.rs)
    against the now-registered overload, re-parking on the
    next-earliest sibling if its pick doesn't admit. The
    legacy `park_pending_and_redispatch` call site in
    [`finish.rs:64`](../../src/machine/execute/scheduler/finish.rs)
    needs a stateful equivalent — investigate during this
    sub-step.
  - **(4e) Cycle-detection guard confirmation.** The drain-end
    guard in
    [`execute`](../../src/machine/execute/scheduler/execute.rs)
    scans for parked nodes after the work queues empty and
    surfaces `SchedulerDeadlock { sample }`. Confirm the
    `sample` reporting (source expression of the first parked
    Dispatch) reads the `working_expr` from
    `KeywordedState` correctly. Add a regression test if the
    existing slate doesn't already cover this carrier shape.

- **Inline vs through-loop discipline — decided.**
  Per-edge state updates (counter decrement, slot splice) run
  inline during the producer's notify-walk in `Scheduler::
  finalize`. Track continuations (admission re-attempt,
  finalize-bind) run *only* when the woken slot pops. Don't fire
  continuations inline — race risk if a continuation runs before
  `pending_deps` reaches zero, or before sibling notify-walks
  finish.

- **`recent_wakes` consumption — decided.** Drain (take, not
  peek) on consumer pop in `run_dispatch_stateful`. A stale wake
  re-firing a continuation is a real risk if drained lazily.

- **`pre_subs` ownership across transition — decided.** Per the
  `Initialized`-embedding rule reified in
  [`dispatch_state.rs`](../../src/machine/execute/scheduler/dispatch_state.rs):
  `KeywordedState { init: prev_initialized, … }` carries the
  `pre_subs` Vec structurally. The submission-time install in
  `add_with_chain` populates it; the `Initialized → Keyworded`
  transition moves the whole `Initialized` struct by value. No
  manual field copy.

- **Risks — open.**
  - **Sub-step sequencing.** 4a first means many tests pass on
    day one of keyworded work. Skipping straight to 4c/4d
    leaves wide test gaps until the one-shot path catches up.
  - **Track-completion ordering.** If two tracks complete in
    the same notify-walk batch, the slot pops once with both
    discharged. The continuation dispatch needs to handle
    "multiple tracks just hit zero" — order matters when the
    bare-name track's re-admission spawns the eager-subs track
    fresh, but pre-existing eager subs would have been
    scheduled at a different time. Sequence the inline updates
    so the dispatch on pop sees consistent state.
  - **Iteration likely.** `Keyworded` is the most complex
    variant. The named-field track layout (per the carrier shape
    in [`dispatch_state.rs`](../../src/machine/execute/scheduler/dispatch_state.rs))
    may need revision — the variant boundary keeps any such
    revision local.

- **Acceptance criteria — decided.** After 4a–4e, full
  `cargo test` toggle-on is green. Add focused tests in
  [`scheduler/tests/dispatch_shapes.rs`](../../src/machine/execute/scheduler/tests/dispatch_shapes.rs)
  for any continuation path not already covered.

## Dependencies

**Requires:**

- [Stateful dispatch — Step 3: fast-lane variants](stateful-dispatch-03-fast-lane-variants.md)

**Unblocks:**

- [Stateful dispatch — Step 5: cutover](stateful-dispatch-05-cutover.md)
- [Fast-lane Bind inlining](fast-lane-bind-inlining.md)

Composes with — but does not block —
[SCC-aware dispatcher for parameterized self-recursive
types](scc-aware-dispatcher-for-self-recursive-types.md): the SCC
threading carrier slots naturally into `KeywordedState`'s field
set, but the SCC item can ship against the legacy driver if step 4
slips.
