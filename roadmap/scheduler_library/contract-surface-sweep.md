# Workgraph contract-surface sweep

**Problem.** The `workgraph` public surface carries items no consumer uses. A
superseded bundled-witness relocation path is still exported with zero live
callers — `UnionWitness`
([witnessed.rs](../../workgraph/src/witnessed.rs)), `Sealed::transfer_into`
(the method in `Sealed`'s `W: Witness` impl block, distinct from the live
`Delivered::transfer_into`), and the bare `Witnessed::merge`, whose only
caller is that dead `transfer_into` — while every live relocation routes
`Delivered::transfer_into` / `merge_composed`. (To verify liveness: every
koan `.transfer_into` call passes a `Residence` argument, which only
`Delivered`'s signature takes.) Several doc comments still cite
`Sealed::transfer_into` as the conceptual relocation seam.
`Workload::Payload` carries a `Clone` bound no scheduler code exercises
([workload.rs](../../workgraph/src/scheduler/workload.rs)). `HostedSetRef` is
re-exported although its own doc says it is not
([carrier.rs](../../workgraph/src/witnessed/carrier.rs)). Facades with no
external caller are `pub`: `Scheduler::resolve_alias` and
`Scheduler::add_park_edge` in
[splice.rs](../../workgraph/src/scheduler/splice.rs), `Deps::park_count`,
`Deps::is_empty`, and `DepResults::len`/`is_empty` in
[deps.rs](../../workgraph/src/scheduler/deps.rs), `AllocViews` in
[step_ctx.rs](../../workgraph/src/witnessed/step_ctx.rs), and `FamilyList`
in [region.rs](../../workgraph/src/witnessed/region.rs). The reach-set
white-box introspection koan touches only from `#[cfg(test)]`
(`RegionSet::fold_omitting`/`members`,
`Carrier::borrows_host`/`with_reach`/`reach_covers`) sits on the production
surface rather than behind the `test-hooks` gate the scheduler's own test
pokes use. `Scheduler::take_for_run` and `Scheduler::take_handoff` are
separate entry points always called back-to-back on the same id — the run
loop's only two callers sit a few lines apart in
[run_loop.rs](../../src/machine/execute/run_loop.rs)'s `execute` loop.

**Acceptance criteria.**

- The bundled-witness relocation path is gone: no `UnionWitness`, no
  `Sealed::transfer_into`, no bare `Witnessed::merge`; doc comments cite
  `Delivered::transfer_into` as the relocation seam.
- `Workload::Payload` carries no `Clone` bound.
- `HostedSetRef` is not exported from `witnessed`.
- `Scheduler::resolve_alias`, `Scheduler::add_park_edge`, `Deps::park_count`,
  `Deps::is_empty`, `DepResults::len`/`is_empty`, `AllocViews`, and
  `FamilyList` are `pub(crate)` or private.
- Reach-set white-box introspection (`RegionSet::fold_omitting`/`members`,
  `Carrier::borrows_host`/`with_reach`/`reach_covers`) is reachable only under
  `cfg(any(test, feature = "test-hooks"))`.
- `take_for_run` returns the node together with the TCO handoff; a separate
  `take_handoff` entry point no longer exists.
- The Miri slate names no test whose sole purpose was pinning a deleted verb
  (the miri skill's delete-vs-whitelist rule decides survivors).

**Directions.**

- *`Witnessed::with` / `Witnessed::map` disposition — open.* Both
  bundled-witness readers have no koan runtime caller, but their doctests are
  load-bearing soundness guards. (a) Keep `pub` as the documented conceptual
  core; (b) demote to `pub(crate)` and re-home the doctests. Recommended: (a).
- *`StepContext::alloc`/`alloc_with` naming — open.*
  [design/scheduler-library.md](../../design/scheduler-library.md) names the
  bare forms as the canonical step surface while koan uses only the `_handle`
  veneers: (a) re-point the design doc at the handle forms; (b) route koan
  through the bare forms. Resolve deliberately rather than blind-shrink.

## Dependencies

**Requires:** none — a mechanical sweep over shipped surface.

**Unblocks:**

- [Scheduler-owned frame storage](scheduler-owned-frame-storage.md) — the
  `take_for_run` fusion front-loads the protocol seam that item extends.
- [Publishing the workgraph crate](workgraph-extraction.md) — the frozen
  surface should not include dead API.
