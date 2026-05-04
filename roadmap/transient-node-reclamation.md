# Transient-node reclamation — shipped

**Problem.** TCO's slot reuse covers only the outermost user-fn frame.
[`Scheduler`](../src/execute/scheduler.rs)'s `nodes`/`results` vecs grew per iteration
whenever a body-internal sub-expression spawned a sub-`Dispatch`/`Bind`. Realistic
recursion (the predicate computation in an `IF`-guarded base case, or a recursive
call's argument expressions) accumulated entries.

**Resolution.** Two-trigger reclamation, all contained in
[`src/execute/{scheduler,nodes,run,finalize}.rs`](../src/execute/) — `BuiltinFn`
unchanged.

- A `node_dependencies: Vec<Vec<usize>>` sidecar on `Scheduler` records each
  `Bind`/`Aggregate`/`AggregateDict` slot's owned sub-slot indices at `add()`-time
  (the deps would otherwise vanish when `take()` consumes the work in the execute
  loop).
- A `free_list: Vec<usize>` of recyclable indices. `add()` pulls from the free-list
  before extending the vecs, so reclaimed slots get reused across iterations.
- `Scheduler::free` walks `Forward` chain links and drains the dep sidecar
  recursively, defensively skipping any still-live slot (`nodes[i].is_some()`).
- Trigger 1: end of `run_bind` / `run_aggregate` / `run_aggregate_dict`. Once deps
  have been read and spliced (or deep-cloned into the result), the dep slots are
  free. The splice references survive `results[dep] = None` because the underlying
  `KObject` lives in an arena, not in the result slot.
- Trigger 2: chain-free at finalize. When `finalize_ready_frames` collapses a
  frame-holder's `Forward(target)` into a `Value`, the chain target is freed —
  recursively walking forward links and stopping at the next frame-holder.

The dependency on monadic-side-effects in the original entry was an
engineering-cost argument (avoid touching `BuiltinFn` twice), not a hard technical
blocker — reclamation turned out to be entirely scheduler-internal.

**Verification.**
- [`tail_recursive_match_keeps_scheduler_bounded`](../src/dispatch/builtins/fn_def.rs)
  drives 10 user-fn calls whose body has a sub-expression (`MATCH (b) WITH …`) and
  asserts `sched.len() ≤ 28` (empirically 22). Without reclamation: ~70+.
- [`free_reclaims_bind_subtree_and_forward_chain`](../src/execute/scheduler.rs)
  and [`free_skips_live_slot_and_is_idempotent`](../src/execute/scheduler.rs) cover
  the unit-level invariants.

**Open follow-up.** Top-level `add_dispatch` slots persist with `Forward(B_call)`
to the user-fn's lifted Value — the user reads through the chain. Each top-level
call therefore costs two persistent slots (the entry slot + its forward target);
collapsing this into a direct Value would need either path compression in
`read_result` or a post-execute pass. Not load-bearing: linear in call count, not
multiplicative in body size.

An unrelated UAF surfaced while writing the test: recursive user-fns taking a
`Tagged` parameter segfault during the test rig's teardown (binary works fine).
Documented as a separate item in
[Open issues from the leak-fix audit](leak-fix-audit.md).
