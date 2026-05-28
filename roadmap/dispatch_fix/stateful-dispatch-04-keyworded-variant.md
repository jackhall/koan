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

Sub-steps 4a + 4b + 4c + 4d have landed: the one-shot path (Resolved
with no parks and no eager subs) terminalizes directly via
`Scheduler::stateful_keyworded_initial`; the Resolved-with-eager-
subs and `Deferred` paths now install an `EagerSubsTrack` on
`KeywordedState` rather than allocating a `NodeWork::Bind` hop; the
Resolved-with-parked-bare-names path now installs a
`BareNameParkTrack` on `KeywordedState` (via
`stateful_install_bare_name_park`) rather than rebuilding the slot
as `DispatchState::initialized` through the legacy
`install_combined_park`; and the `ResolveOutcome::ParkOnProducers`
arm of `stateful_keyworded_initial` (plus the post-eager-subs
re-resolve in `stateful_keyworded_finish`) now installs an
`OverloadParkTrack` on `KeywordedState` (via
`stateful_install_overload_park`) rather than calling the legacy
`park_pending_and_redispatch`. On eager-subs completion
`stateful_keyworded_resume_eager_subs` reads each sub's terminal,
splices `Future(obj)` into `working_expr.parts[i]`, frees the subs,
and `stateful_keyworded_finish` re-resolves dispatch against the
spliced expression — the re-resolve is authoritative even when the
initial pre-eager pick succeeded, so an element-typed `Future(_)`
that narrows the typed-slot admission surfaces `DispatchFailed`
(non-match) rather than a bind-time `TypeMismatch`, matching the
legacy `run_bind` surface. On bare-name-park completion
`stateful_keyworded_resume_bare_name_park` re-runs
`stateful_keyworded_initial` against the carried `working_expr` and
preserved `pre_subs`; the producers' now-bound values surface
through the rebuilt `bare_outcomes` cache and the wrap-slot splice
fires `Future(obj)` for them on the second pass. On overload-park
completion `stateful_keyworded_resume_overload_park` re-runs
`stateful_keyworded_initial` against the carried `expr` and
preserved `pre_subs`; the producers' now-finalized state (an
overload registered in `bindings.functions`, or a resolved bare
name) feeds the rebuilt resolve. The legacy
`park_pending_and_redispatch` / `install_combined_park` remain
alive only for the toggle-off `run_dispatch` driver and the
`run_bind` re-park; their last callers go away in step 6. With the
third track field added, the `Keyworded` variant of `DispatchState`
is boxed (`Keyworded(Box<KeywordedState<'a>>)`) so the `NodeWork` /
`NodeStep` / `Node` / `SlotState` cascade stays under clippy's
`large_enum_variant` threshold; the box pays one allocation per
parked Keyworded slot (a rare path) and Step 5/6 cutover may
consolidate the three `Option<Track>` fields into a single
`Option<KeywordedTrack>` enum at which point the box can come back
out.

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
  - **(4b) Eager-subs track + Deferred fold. Shipped.**
    `EagerSubsTrack` lives on `KeywordedState`; the Resolved-with-
    eager-subs and `Deferred` arms install it through
    `stateful_install_eager_subs_track` and park the slot on its
    subs as Owned dep_edges. On track completion
    `stateful_keyworded_resume_eager_subs` reads each terminal,
    splices `Future(obj)` into `working_expr.parts[i]`, frees the
    subs, and `stateful_keyworded_finish` re-resolves dispatch
    against the spliced expression — the re-resolve is
    authoritative even when the initial pre-eager pick succeeded,
    so an element-typed `Future(_)` that narrows the typed-slot
    admission surfaces `DispatchFailed` (non-match) rather than a
    bind-time `TypeMismatch`. No `Bind` slot allocation on the
    stateful path — the keyworded re-resolve replaces what
    today's `run_bind` does, eliminating the per-call Bind hop
    the legacy driver pays. Per-edge inline splice was deferred
    in favor of at-pop splice (the slot pops exactly once with
    `pending_deps == 0`, so all subs are terminal at resume time).
  - **(4c) Bare-name park track.** `bare_name_park: Some(Track
    { producers, splice_indices })`. Equivalent of today's
    `install_combined_park` folded into the variant's state.
    Cycle check via `DepGraph::would_create_cycle` runs at
    track-installation time, same surface today's fused walk
    uses. Re-admission continuation re-attempts strict
    admission against the now-bound types.
  - **(4d) Overload park track. Shipped.** `OverloadParkTrack`
    lives on `KeywordedState`; the `ResolveOutcome::ParkOnProducers`
    arm of `stateful_keyworded_initial` and the post-eager-subs
    re-resolve in `stateful_keyworded_finish` install it through
    `stateful_install_overload_park` and park the slot on the
    surviving producers as `Notify` dep_edges. The same install
    serves both `ParkOnProducers` sub-cases —
    `resolve_dispatch_with_chain` surfaces bare-name `Placeholder`s
    and innermost-visible `pending_overloads[key]` entries through
    the same return variant, so one installer covers both. The
    install filters producers for cycles and already-errored
    terminals (mirroring the legacy `park_pending_and_redispatch`
    guards) before installing the park edges; an empty filtered
    list surfaces `DispatchFailed`. On track completion
    `stateful_keyworded_resume_overload_park` re-runs
    `stateful_keyworded_initial` against the carried `expr` and
    preserved `pre_subs`; if the rebuilt resolve fires
    `ParkOnProducers` again (a later sibling now-visible at the
    bucket but still in flight), the re-entry installs a fresh
    `overload_park` on the next-earliest sibling — matching the
    legacy re-park behavior. The `finish.rs:64` call site in
    `run_bind` stays as-is: the bind's re-Dispatch goes through
    `Initialized → classify → Keyworded → stateful_keyworded_initial`
    on wake, where the new stateful installer takes over on any
    subsequent `ParkOnProducers`. Step 6 folds the
    `Bind → Dispatch` transition directly into the stateful state
    machine. With the third `Option<Track>` field added,
    `KeywordedState` exceeded clippy's `large_enum_variant`
    threshold for the `NodeWork` / `NodeStep` / `Node` /
    `SlotState` cascade, so `DispatchState::Keyworded` is now
    boxed (`Keyworded(Box<KeywordedState<'a>>)`); the box pays one
    allocation per parked Keyworded slot (a rare path — the
    fast-lane variants never construct `Keyworded`, and the
    one-shot Keyworded path terminalizes without installing a
    track). Step 5/6 may consolidate the three `Option<Track>`
    fields into a single `Option<KeywordedTrack>` enum at which
    point the box can come back out.
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
