# Fast-lane Bind inlining

Apply the [Step 4 `KeywordedState`](stateful-dispatch-04-keyworded-variant.md)
Track-based eager-subs inlining pattern to the remaining fast-lane
variants that still build a `NodeWork::Bind` slot for their eager
sub-Dispatches. After this item ships, no `DispatchShape` variant
spawns a Bind slot — `run_bind` and `NodeWork::Bind` become dead and
can be removed alongside the legacy `run_dispatch` deletion in
[Step 6](stateful-dispatch-06-deletion.md).

**Problem.** Step 4 eliminates the Bind slot for the keyworded path
by folding the wait-and-rebind into `KeywordedState`'s eager-subs
Track plus an inline `f.bind + invoke_to_step` continuation. The
fast-lane variants still go through Bind:

- `FunctionValueCall.schedule_picked_eager`
  ([`dispatch.rs:1069`](../../src/machine/execute/scheduler/dispatch.rs))
  builds `NodeWork::Bind { expr, subs }` for any eager part of the
  kwarg-reconstructed expression.
- `ConstructorCall` routes through `newtype_construct` /
  `dispatch_constructor` which produce `BodyResult::DeferTo(combine_id)`
  — that path uses a Combine slot, not Bind, and is out of scope.
  Struct / Tagged construction's `struct_value::apply` /
  `tagged_union::apply` return `Tail` shapes that don't need eager
  scheduling at this level.

The remaining Bind callers after Step 4 are therefore
`FunctionValueCall` only.

**Impact.**

- Removes the last Bind spawn site on the stateful driver. Once the
  legacy `run_dispatch` is deleted in
  [Step 6](stateful-dispatch-06-deletion.md), `run_bind` and
  `NodeWork::Bind` have no callers — both can be deleted in the
  same commit.
- One fewer slot per eager-bearing fast-lane call: the Dispatch
  parks directly on its eager subs (Track-based) and binds inline
  on completion. Saves an alloc per call.
- Consolidates the wait-and-rebind pattern under one mechanism
  (`KeywordedState.eager_subs` semantics, generalized) — easier to
  reason about and to extend (e.g. the SCC threading carrier).

**Directions.**

- **Generalize `KeywordedState` or add a sibling variant — open.**
  Two shapes to consider:
  - **Reuse `KeywordedState`** with `function: Some(&KFunction)`
    pre-populated and `bare_name_park` / `overload_park` always
    `None`. The fast-lane case is structurally a subset of the
    keyworded eager-subs path (Resolved picked, no bare-name
    machinery), so the same state struct + continuation fits. Wins:
    one machinery to maintain. Loses: `KeywordedState`'s name no
    longer matches its uses; bare-name-irrelevant fields ride along.
  - **Add `FnValueState` real fields** (today it's a stub embedding
    `Initialized`). Mirror just the `eager_subs` track from
    `KeywordedState` — no bare-name / overload tracks needed because
    the fast lane already filters those at classification time.
    Keeps the per-variant separation Step 1's envelope was designed
    for; small duplication of the eager-subs handler.

  Recommendation: per-variant struct (`FnValueState` grows real
  fields) — matches the Step 1 envelope intent. Decide at
  implementation time after Step 4 lands and the keyworded
  eager-subs continuation's shape is concrete.

- **`run_bind` retirement — decided as follow-up.** After this item
  ships, the only `run_bind` callers are on the toggle-off legacy
  `run_dispatch` path. Step 6's legacy deletion removes the last
  call site and `run_bind` plus `NodeWork::Bind` can be deleted in
  the same commit.

- **Acceptance criteria — decided.** Toggle-on `cargo test` green.
  Under toggle-on, the only `NodeWork::Bind` spawns come from
  legacy `run_dispatch` (toggle-off path) until Step 6 deletes
  legacy; production hot path spawns zero Bind slots.

- **Risks — open.**
  - Frame-label drift. `run_bind` uses `<bind>` for dep-error
    propagation. Whichever fast-lane inlines the wait must surface
    the same frame label on dep-error to keep error tests stable.
  - Per-variant churn vs `KeywordedState` reuse. The shape decision
    above is reversible per variant — defer to implementation time.

## Dependencies

**Requires:**

- [Stateful dispatch — Step 4: `Keyworded` variant](stateful-dispatch-04-keyworded-variant.md)

**Unblocks:**

- `run_bind` / `NodeWork::Bind` deletion (folds into
  [Step 6: deletion](stateful-dispatch-06-deletion.md)).
