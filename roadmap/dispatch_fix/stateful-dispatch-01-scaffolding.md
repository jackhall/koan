# Stateful dispatch — Step 1: scaffolding

Introduce the `DispatchState` enum, the new `run_dispatch_stateful`
entry point, and the per-`Scheduler` routing toggle that selects
between the legacy and stateful drivers. No behavior change: the
stateful entry point delegates to the legacy `run_dispatch` for
every variant until later steps implement per-variant handlers.

**Problem.**
[`run_dispatch`](../../src/machine/execute/scheduler/dispatch.rs)
recomputes its full intermediate state on every invocation. A
`Dispatch` slot that parks (on a placeholder bare name, on a pending
overload bucket, on an eager sub-Dispatch result) is woken by the
notify-walk and re-enters `run_dispatch`, which re-classifies the
shape, rebuilds the `bare_outcomes` cache, re-runs strict admission,
and re-walks every part. The redundancy is structural; only
`pre_subs` survives across the rewrite because re-allocating
already-submitted sub-Dispatches leaks them. Closing the redundancy
requires a state-bearing dispatch slot, and that change has to land
behind a toggle: the live dispatch path is the busiest hot path in
the engine, and switching it in one PR is too high-risk.

**Impact.**

- The `DispatchState<'a>` enum exists and rides inside
  `NodeWork::Dispatch`, with one variant per `DispatchShape`
  (`Initialized`, `BareIdentifier`, `BareTypeLeaf`,
  `TypeConstructorCall`, `FunctionValueCall`, `SigiledTypeExpr`,
  `Keyworded`). Later steps implement the per-variant handlers
  without redefining the carrier.
- A `Scheduler::use_stateful_dispatch` toggle (default `false`)
  routes between `run_dispatch` and `run_dispatch_stateful` per
  call. Tests can flip the toggle to exercise the new driver;
  production stays on the legacy path until [step 5](stateful-dispatch-05-cutover.md).
- The architectural decisions from the design discussion are
  materialized in code shape: variant-per-shape envelope, per-
  variant struct carrying its own state, the `Initialized` variant
  as the universal birth state, the `Initialized` struct embedded
  by value inside per-variant state structs (so `pre_subs` ridealong
  is type-system-enforced rather than convention).

**Directions.**

- **`DispatchState` shape — decided.** Enum with one variant per
  `DispatchShape`, each carrying a per-variant state struct that
  embeds `Initialized` by value:
  ```rust
  enum DispatchState<'a> {
      Initialized(Initialized),
      BareIdentifier(BareIdState<'a>),
      BareTypeLeaf(BareTypeState<'a>),
      TypeConstructorCall(TyCtorState<'a>),
      FunctionValueCall(FnValueState<'a>),
      SigiledTypeExpr(SigilState<'a>),
      Keyworded(KeywordedState<'a>),
  }
  struct Initialized { pre_subs: Vec<(usize, NodeId)> }
  struct KeywordedState<'a> { init: Initialized, /* … */ }
  ```
  The `init: Initialized` field on each per-variant struct makes
  `pre_subs` ride along structurally — dropping it requires an
  explicit destructure-and-discard rather than a silent oversight.

- **Routing toggle — decided.** A `use_stateful_dispatch: bool`
  field on `Scheduler` (default `false`). The dispatch arm in
  [`Scheduler::execute`](../../src/machine/execute/scheduler/execute.rs)
  branches on it to call either `run_dispatch` or
  `run_dispatch_stateful`. Tests opt in via
  `Scheduler::with_stateful_dispatch(true)` (constructor builder)
  or `KOAN_STATEFUL_DISPATCH=1` env var. The legacy path remains
  the production default until step 5.

- **`run_dispatch_stateful` stub — decided.** Initial body:
  classify the shape, transition `Initialized → <variant>`, then
  for every variant delegate immediately to the legacy
  `run_dispatch` for the actual step. Later steps replace each
  variant's delegation with a real handler.

- **Files affected — decided.**
  - [`src/machine/execute/nodes.rs`](../../src/machine/execute/nodes.rs):
    extend `NodeWork::Dispatch` from `{ expr, pre_subs }` to
    `{ expr, state: DispatchState<'a> }`. The `state` field's
    `Initialized.pre_subs` replaces the standalone `pre_subs`
    field 1:1.
  - New file `src/machine/execute/scheduler/dispatch_state.rs`:
    `DispatchState` enum, per-variant structs with empty
    payloads (filled by later steps).
  - [`src/machine/execute/scheduler/dispatch.rs`](../../src/machine/execute/scheduler/dispatch.rs):
    add `run_dispatch_stateful` entry point.
  - [`src/machine/execute/scheduler.rs`](../../src/machine/execute/scheduler.rs)
    and
    [`src/machine/execute/scheduler/execute.rs`](../../src/machine/execute/scheduler/execute.rs):
    add the toggle, route the dispatch arm.
  - [`src/machine/execute/scheduler/submit.rs`](../../src/machine/execute/scheduler/submit.rs):
    `add_with_chain` constructs `NodeWork::Dispatch { state:
    DispatchState::Initialized(Initialized { pre_subs }) }` instead
    of the old struct shape.

- **Acceptance criteria — decided.** `cargo test` green with
  toggle off (production default). `cargo test` green with
  toggle on (the stub-delegate path runs through every test). No
  new behavioral coverage in this step.

## Dependencies

**Requires:** none.

**Unblocks:**

- [Stateful dispatch — Step 2: `recent_wakes` side-channel](stateful-dispatch-02-recent-wakes.md)
