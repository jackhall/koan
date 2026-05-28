# Stateful dispatch — Step 6: deletion

Delete the legacy `run_dispatch`, the rewrite paths it relied on
(`install_combined_park`, `park_pending_and_redispatch`,
`schedule_eager_only`, `schedule_picked_eager`, `PendingSub`), and
the `Scheduler::use_stateful_dispatch` toggle. Migrate the
architectural narrative carried in
[`dispatch_state.rs`](../../src/machine/execute/scheduler/dispatch_state.rs)'s
module-header comment (variant-per-shape envelope, `Initialized`
embedding rule, per-variant struct layout) into
`design/execution-model.md` as shipped behavior.

**Problem.** After step 5, production dispatch runs on the stateful
driver but the legacy `run_dispatch` body, the rewrite paths it
relied on, and the routing toggle all remain in the source tree as
dead-but-reachable code. The toggle is a maintenance liability:
every change to the dispatch path has to consider both branches;
two implementations of the same hot path drift; the rewrite paths
(`install_combined_park`, `park_pending_and_redispatch`) read
identically to their stateful equivalents but live in different
modules. The architectural narrative for the new driver lives in
six roadmap files; with all six shipped, that narrative belongs in
the design tree.

**Impact.**

- The dispatch path has one implementation. Changes affect one
  body, one set of helpers, one test corpus. Drift between
  driver versions is structurally impossible after this step.
- The `install_combined_park`, `park_pending_and_redispatch`,
  `schedule_eager_only`, `schedule_picked_eager` helper module
  surface in
  [`scheduler/dispatch.rs`](../../src/machine/execute/scheduler/dispatch.rs)
  contracts: the slot-state model subsumes them.
- [`design/execution-model.md`](../../design/execution-model.md)
  describes the dispatcher's actual production shape rather than
  pointing at six roadmap files for the future-tense narrative.
- The shared helpers retained
  (`classify_dispatch_shape`, `resolve_name_part`,
  `propagate_dep_error`, `bare_name_of`,
  `extract_named_call_inner`) become the dispatcher's stable
  external surface; the stateful driver in
  `dispatch_state.rs` becomes the body.

**Directions.**

- **Deletion targets — decided.**
  - [`src/machine/execute/scheduler/dispatch.rs`](../../src/machine/execute/scheduler/dispatch.rs):
    delete `run_dispatch`, `install_combined_park`,
    `park_pending_and_redispatch`, `schedule_eager_only`,
    `schedule_picked_eager`, the `PendingSub` enum, and the
    fast-lane handlers (`fast_lane_bare_identifier`,
    `fast_lane_bare_type_leaf`, `fast_lane_function_value_call`,
    `fast_lane_sigiled_type_expr`, `fast_lane_type_constructor_call`).
    Keep `classify_dispatch_shape`, `resolve_name_part`,
    `propagate_dep_error`, `bare_name_of`,
    `extract_named_call_inner` — they're shared by the stateful
    driver.
  - [`src/machine/execute/scheduler.rs`](../../src/machine/execute/scheduler.rs)
    and
    [`src/machine/execute/scheduler/execute.rs`](../../src/machine/execute/scheduler/execute.rs):
    remove the `use_stateful_dispatch` field, the builder, the
    env-var hook, and the dispatch-arm branch. The arm calls
    `run_dispatch_stateful` directly.
  - Rename `run_dispatch_stateful` to `run_dispatch` (claiming
    the canonical name back).

- **Call-site sweep — decided.**
  [`scheduler/finish.rs:64`](../../src/machine/execute/scheduler/finish.rs)'s
  `park_pending_and_redispatch` call must move to a stateful
  equivalent. The shape was investigated during the Keyworded
  variant work (the legacy mutator stayed alive for the
  `run_bind` re-park surface); confirm the migration here. Any
  other call site of the deleted helpers
  surfaces through `cargo build` failures; resolve each by
  routing to the stateful equivalent or removing if dead.

- **Documentation migration — decided.** The architectural
  narrative reified in
  [`dispatch_state.rs`](../../src/machine/execute/scheduler/dispatch_state.rs)'s
  module-header comment — `DispatchState` shape, the `Initialized`
  embedding rule, hybrid drive-forward partition, `recent_wakes`
  side-channel — moves into `design/execution-model.md`'s
  § run_dispatch section as the description of the shipped
  dispatcher. The `## Open work` entry that pointed at this chain
  is removed. Run the documentation skill before applying the doc
  edits.

- **`rm-roadmap` chain — decided.** As each step in the chain
  ships, run
  `python3 tools/doclinks.py rm-roadmap roadmap/dispatch_fix/stateful-dispatch-NN-<slug>.md`
  to remove its file and prune ROADMAP.md / inbound bullets.
  This step finishes the chain by deleting its own roadmap
  file last; the migration into `design/execution-model.md`
  completes the partition.

- **Acceptance criteria — decided.** `cargo test` green;
  `cargo clippy --fix` clean on the touched files; Miri full
  leak slate green; `python3 tools/doclinks.py check` passes
  cleanly after the doc migration.

## Dependencies

**Requires:**

- [Stateful dispatch — Step 5: cutover](stateful-dispatch-05-cutover.md)
- [Fast-lane Bind inlining](fast-lane-bind-inlining.md) — deletes
  `schedule_picked_eager` / `PendingSub`, which the fast-lane
  `FunctionValueCall` path still uses today.

**Unblocks:** none.
