# Hoist dispatcher out of scheduler behind a typed facade

Move the `scheduler::dispatch` subtree up to `execute::dispatch` as a
sibling of `execute::scheduler`, and replace the `&mut Scheduler<'a>`
parameter every dispatch entry point carries with a typed facade that
names exactly the scheduler operations the dispatcher needs.

**Problem.** The dispatch tree lives at
[`src/machine/execute/scheduler/dispatch/`](../../src/machine/execute/scheduler/dispatch.rs),
nested inside the scheduler module. The nesting buys it
`pub(in crate::machine::execute::scheduler::dispatch)` visibility,
which is the only thing letting the dispatcher's wide reach into the
scheduler compile:

1. **Field reads of scheduler substructures.**
   `sched.deps.{add_owned_edge, add_park_edge, would_create_cycle,
   clear_dep_edges}`
   ([`dep_graph.rs`](../../src/machine/execute/scheduler/dep_graph.rs))
   and `sched.store.take_recent_wakes`
   ([`node_store.rs`](../../src/machine/execute/scheduler/node_store.rs))
   are reached through `pub(super)` field access from inside the same
   subtree. The dispatcher names the storage layout (`DepGraph` +
   `NodeStore` as separate members) directly.
2. **Ambient-state reads.** `sched.active_chain.as_deref()` is read at
   every name-resolution and bare-outcomes build site
   ([`keyworded.rs`](../../src/machine/execute/scheduler/dispatch/keyworded.rs),
   [`fn_value.rs`](../../src/machine/execute/scheduler/dispatch/fn_value.rs),
   [`single_poll.rs`](../../src/machine/execute/scheduler/dispatch/single_poll.rs));
   the external [`SchedulerHandle::current_lexical_chain`](../../src/machine/core/kfunction/scheduler_handle.rs)
   accessor exists but isn't used internally because the dispatcher
   holds `&mut Scheduler` directly.
3. **Dispatcher-only methods on `Scheduler`.**
   `build_bare_outcomes`, `install_eager_subs`,
   `replace_with_parked_dispatch`, `resume_eager_subs`
   ([`dispatch.rs`](../../src/machine/execute/scheduler/dispatch.rs))
   plus `invoke_to_step` / `invoke_to_step_pinned`
   ([`finish.rs`](../../src/machine/execute/scheduler/finish.rs))
   and `defer_to_lift`
   ([`execute.rs`](../../src/machine/execute/scheduler/execute.rs))
   are `pub(in crate::machine::execute::scheduler::dispatch)` methods
   that exist solely to be called from the dispatch tree. They are
   logically dispatcher operations but spelled as `impl Scheduler` so
   they can name `self.deps` / `self.store` / the active-frame fields.

The external builtin-facing surface
[`SchedulerHandle`](../../src/machine/core/kfunction/scheduler_handle.rs)
is already narrow (eight methods, no field access). The dispatcher's
surface is wide *and* untyped *and* enforceable only by nesting
convention: a future dispatcher addition can reach for any scheduler
field or any private method without tripping a visibility wall, and
any scheduler refactor (splitting `DepGraph`, renaming
`active_chain`, moving `take_recent_wakes`) sweeps the dispatch tree
to find callers. Sibling subtrees `execute::interpret` and
`execute::lift` are scheduler peers; `execute::dispatch` is the
remaining peer still living inside.

**Impact.**

- The dispatch tree becomes a sibling subtree under
  `execute::dispatch`, matching `execute::interpret` and
  `execute::lift`. The "is this scheduler-internal or dispatcher-only?"
  question maps onto the file tree.
- The `pub(in crate::machine::execute::scheduler::dispatch)` visibility
  crutch disappears: every scheduler touch the dispatcher makes goes
  through the new facade, and the compiler is what enforces it.
  Adding a fast-lane shape or a `DispatchState` variant lists its
  scheduler operations against the typed surface.
- `DepGraph`, `NodeStore`, and the active-frame fields stop being
  part of the dispatch tree's spelled API: a future `DepGraph` split
  or `active_chain` rename is a single-file change inside
  `scheduler/`.
- `invoke_to_step` / `invoke_to_step_pinned` / `defer_to_lift` /
  `build_bare_outcomes` / `install_eager_subs` /
  `replace_with_parked_dispatch` / `resume_eager_subs` move to the
  facade alongside the field-access methods they bridge to, dropping
  the cross-file `pub(in ...::dispatch)` method declarations in
  `dispatch.rs`, `finish.rs`, and `execute.rs`.
- The [collapse-keyword-free-dispatch](collapse-keyword-free-dispatch-into-dispatch-rs.md)
  refactor's per-shape file split lands in the hoisted
  `execute::dispatch/` rather than depth-6 under
  `scheduler/dispatch/`, and each new shape file imports the facade
  rather than `Scheduler` directly.

**Directions.**

- **Hoist destination — decided.** `src/machine/execute/dispatch/`,
  sibling of `execute::scheduler` and `execute::interpret`. The
  existing `scheduler/dispatch.rs` becomes `execute/dispatch.rs`;
  the `dispatch/{keyworded, fn_value, single_poll, tests, …}` subdir
  rides along. `NodeWork::Dispatch` in `execute::nodes` already
  references `DispatchState` through `pub(in crate::machine::execute)`,
  so the move is a same-crate-level reparenting. **Further hoisting
  was probed and rejected:** under `modgraph_rewrite module --rename`
  against the fresh baseline (231.75), `execute::dispatch` scores
  229.11 (Δ −2.64), but `machine::dispatch` scores 235.51 (Δ +3.76)
  and `koan::dispatch` scores 232.58 (Δ +0.83). The dispatcher's
  edges to `machine::core::*` and `machine::execute::scheduler::*`
  cross more wrapper boundaries with each additional hoist, adding
  coupling faster than nesting drops; one level up is the local
  optimum.
- **Surface shape — open.** Either (a) a `DispatchCtx<'a, 'b>` wrapper
  struct that holds `&'b mut Scheduler<'a>` (or sub-borrows of its
  fields) and exposes the dispatch-only operations as methods, or (b)
  an `InternalSchedulerHandle<'a>` trait analogous to
  [`SchedulerHandle`](../../src/machine/core/kfunction/scheduler_handle.rs)
  with `impl InternalSchedulerHandle<'a> for Scheduler<'a>`.
  Recommended: wrapper struct, since the dispatcher needs read access
  to `active_chain` *and* mutable access to `deps`, and a struct can
  expose those as disjoint sub-borrows in a way a trait method
  signature can't.
- **Whether to extend `SchedulerHandle` instead — decided against.**
  `SchedulerHandle` is the *external* builtin-body-facing surface,
  defined in `core::kfunction` so `BuiltinFn` can name it without
  `kfunction` importing from `execute`. The dispatcher's needs
  (`DepGraph` mutations, `take_recent_wakes`, dispatch-state
  construction) are internal scheduler concerns that shouldn't widen
  every builtin author's API.
- **Scope of the facade — decided.** Full move: relocate
  `build_bare_outcomes`, `install_eager_subs`,
  `replace_with_parked_dispatch`, `resume_eager_subs`,
  `invoke_to_step{,_pinned}`, `defer_to_lift` onto the facade so the
  dispatcher's home for these operations is one type. The
  field-access-only variant leaves the cross-module `pub(in ...)`
  methods in place and doesn't earn the hoist's partition win.
- **Scoring — partially measured.** The pure-rename simulation above
  captures the reparenting (Δ −2.64 at `execute::dispatch`) but not
  the facade's effects: relocating `build_bare_outcomes` /
  `install_eager_subs` / `replace_with_parked_dispatch` /
  `resume_eager_subs` / `invoke_to_step{,_pinned}` / `defer_to_lift`
  off `impl Scheduler` onto the new facade type should earn
  additional owner credit on `execute::dispatch` and drop the
  cross-module method declarations on `scheduler/{dispatch, finish,
  execute}.rs`. Re-score with `modgraph_rewrite item` once the
  facade type's destination is fixed. Re-check against the
  [collapse-keyword-free-dispatch](collapse-keyword-free-dispatch-into-dispatch-rs.md)
  bundle — whichever ships first invalidates the other's baseline.

## Dependencies

**Requires:** none. The current `pub(in ...::dispatch)` visibility
scope is what makes the dispatcher's reach into `Scheduler` internal;
relocating it up one level and routing through a typed surface is a
local rewrite.

**Unblocks:** none required. The
[collapse-keyword-free-dispatch](collapse-keyword-free-dispatch-into-dispatch-rs.md)
refactor lands more cleanly afterward — each new per-shape file
imports the facade instead of `Scheduler` and lives at
`execute::dispatch/<shape>.rs` rather than depth-6 under
`scheduler/dispatch/` — but does not depend on it. Whichever item
ships first invalidates the other's modgraph baseline and obliges a
re-score before the second.
