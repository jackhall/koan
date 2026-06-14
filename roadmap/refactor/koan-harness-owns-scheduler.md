# KoanHarness owns the scheduler

Introduce a `KoanHarness<'run>` struct that owns the `Scheduler<'run>` and is the sole
holder of `&mut Scheduler` across the execute tree, so "everything outside the harness is
read-only" becomes structurally enforced rather than a naming convention.

**Problem.** The "write harness" is diffuse. The
[decide → outcome → apply contract](../../design/execution-model.md#the-dispatcher--scheduler-boundary)
names [`apply_outcome`](../../src/machine/execute/dispatch/harness.rs) as the sole `&mut
Scheduler` writer, but two other AST-aware `&mut Scheduler` surfaces live on the dispatch
side outside it:

- [`submit_dispatch`](../../src/machine/execute/dispatch/submit.rs) — walks `expr.parts`,
  runs `extract_binder_install`, recursively pre-submits eager argument slots, allocates
  via `submit_node`, and stamps the binder placeholder, all holding `&mut Scheduler`.
- the five literal-lowering functions in
  [`dispatch/literal.rs`](../../src/machine/execute/dispatch/literal.rs)
  (`schedule_{list,dict,record}_literal`, `classify_aggregate_part`,
  `resolve_aggregate_bare_name`) — walk `ExpressionPart` and submit combine slots via
  `combine_here` / `dispatch_here`, holding `&mut Scheduler`.

Both are reached only from `apply_outcome` or the top-level
[`enter_block`](../../src/machine/execute/scheduler.rs) entry, never from a read-only decide
— but nothing structural enforces that. A decide handler could take `&mut Scheduler` and the
type system would not object.

The scheduler also still names AST. `KExpression` appears in six scheduler method
signatures — `enter_block`, `add_dispatch`, `add_dispatch_with_chain`,
`add_dispatch_in_frame`, `dispatch_here`, `dispatch_body_statements` — each a thin wrapper
that resolves `(scope, node_scope, chain)` from scheduler state and forwards to
`dispatch::submit_dispatch`. So the AST-aware submission path is split across scheduler
wrappers, `submit_dispatch`, and `literal.rs`, and the scheduler's public surface is not
AST-free.

**Acceptance criteria.**

- A `KoanHarness<'run>` struct owns the scheduler by composition (a `sched: Scheduler<'run>`
  field, not a `&mut` borrow) and is the only holder of `&mut Scheduler` in the execute
  tree.
- The execute loop, `apply_outcome`, `submit_dispatch`, the literal-lowering functions, and
  the dispatch-submission wrappers (`dispatch_here`, `add_dispatch_in_frame`, `enter_block`,
  `dispatch_body_statements`, `combine_here`) are `&mut self` methods on `KoanHarness`.
- No function under `dispatch/` other than `KoanHarness` takes or holds `&mut Scheduler`;
  every decide handler sees only a `SchedulerView` / `&Scheduler`.
- `Scheduler`'s public surface is AST-free: no method signature names `KExpression` or any
  `machine::model::ast` type. `Scheduler` exposes read views plus the low-level write prims
  (`submit_node`, `alloc_slot`, `add_owned_edge` / `add_park_edge`, `acquire_tail_frame`,
  `free`, `resolve_node_scope`, `ensure_run_frame`, scope / chain reads).
- [`interpret`](../../src/machine/execute/interpret.rs) and the scheduler tests construct a
  `KoanHarness` and drive the run through it.
- Behavior is unchanged: the full `cargo test` slate passes.

**Directions.**

- *`KoanHarness` owns the scheduler by composition, not a `&mut` borrow — decided.* A
  borrowing `KoanHarness<'a, 'run> { sched: &'a mut Scheduler<'run> }` would leave the
  execute loop and the AST-aware submission methods on the `Scheduler` type, so it delivers
  neither the read-only-dispatch nor the AST-free-scheduler win. Owning moves both onto the
  harness type.
- *No plan / materialize split for `submit_dispatch` — decided.* Because the harness is
  allowed to be both `&mut` and AST-aware — it is the one place AST meets graph writes —
  `submit_dispatch` and the literal lowering become harness methods as-is. The read-only-plan
  / `&mut`-materialize split (needed only if submission stayed a free function in a read-only
  `dispatch/`) is unnecessary.
- *The execute loop lives on `KoanHarness` — decided.* It follows from ownership: the loop
  pops slots, runs the decide against a `SchedulerView` over `&self.sched`, and applies the
  outcome through the harness's own `&mut self`.
- *The `Scheduler`-prim vs `KoanHarness`-method boundary — open.* The chain / scope
  resolution helpers (`resolve_node_scope`, `ensure_run_frame`, `ambient_or_detached_chain`)
  could stay on `Scheduler` as AST-free state operations or move onto `KoanHarness`.
  Recommended: keep them on `Scheduler` — they read scheduler state and name no AST, so they
  belong with the store, and the harness calls them through `self.sched`.

## Dependencies

An engine-internal refactor on the shipped scheduler / dispatch substrate; it subsumes the
deferred "make the scheduler's signatures AST-free" cleanup. Update
[design/execution-model.md](../../design/execution-model.md#the-dispatcher--scheduler-boundary)
when the harness type lands.

**Requires:** none — engine-internal.

**Unblocks:** none tracked yet.
