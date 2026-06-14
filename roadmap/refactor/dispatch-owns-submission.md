# Dispatch submission owns binder-install; merge the dispatch node variants

Move binder-install and recursive pre-submission out of the scheduler's generic
submit path into a single dispatch-layer entry point, so no `NodeWork` variant
names a `KExpression` and the scheduler never introspects an AST. With the AST
gone, the two dispatch node variants collapse into one closure-carrying `Decide`.

**Problem.** The scheduler reaches into `KExpression` structure in
[`Scheduler::submit_node`](../../src/machine/execute/scheduler/submit.rs): for
every incoming `NodeWork::Dispatch`, it runs `extract_binder_install` (reads
`expr.untyped_key()`, the `KFunction` `binder_name`/`binder_bucket` extractors,
and signature shape — `submit.rs:28-86`), recursively pre-submits the eager
argument slots as sub-Dispatches (`submit.rs:250-278`), and stamps the binder
placeholder on the scope (`submit.rs:337-351`). The binder *knowledge* already
lives on `KFunction` and the placeholder install is already a `Scope` operation
(`scope.install_placeholder` / `install_pending_overload`); only the orchestration
is mislocated in the scheduler's submit chokepoint. The cost shows up twice:

- `NodeWork::Dispatch { expr, pre_subs }` holds an AST and a submission-time
  side-table; `NodeWork::DispatchResume` is a near-duplicate
  (`SchedulerView -> Outcome` closure) split off only because the birth state
  carries structured `expr` the resume state doesn't
  ([`nodes.rs`](../../src/machine/execute/nodes.rs)). `run_dispatch` (a 12-arm
  `match expr.shape()` classifier) and `run_dispatch_resume` are two handlers for
  what is one operation: run a decide, apply its `Outcome`
  ([`dispatch.rs`](../../src/machine/execute/dispatch.rs)).
- The forward-reference visibility invariant — *"a later sibling that dispatches
  before the binder's slot pops finds the entry and parks rather than surfacing
  `UnboundName`/`DispatchFailed`"*
  ([execution-model.md § Submission-time binder install](../../design/execution-model.md#submission-time-binder-install-and-recursive-sub-dispatch),
  lines 508-511) — is enforced by scheduler code that has no business knowing what
  a binder is.

**Acceptance criteria.**

- No `NodeWork` variant names or holds a `KExpression`. The deadlock carrier is
  already a pre-rendered `String` (shipped); the dispatch birth state's `expr`
  moves into a captured decide closure.
- `Scheduler::submit_node` never pattern-matches `NodeWork::Dispatch { expr }`; it
  is a generic slot allocator over an opaque work value. `extract_binder_install`
  and the recursive pre-submission live in the dispatch layer.
- A single dispatch-layer `submit_dispatch(sched, expr, scope, chain)` entry point
  does binder-install + recursive pre-sub + `scope.install_placeholder` + builds
  the decide closure; every former `add_dispatch_*` / `enter_block` /
  `dispatch_body_statements` Dispatch-submitting site routes through it. Because a
  binder may nest in any subexpression, this entry runs on *every* dispatch
  submission, not just block statements.
- `NodeWork::Dispatch` and `NodeWork::DispatchResume` are one variant (a
  closure-carrying `Decide` plus the `Option<String>` carrier); `run_dispatch` and
  `run_dispatch_resume` are one `run_decide` handler.
- The forward-reference visibility invariant holds unchanged: binder placeholders
  install at submission time, before any sibling slot pops. (The existing
  forward-reference and recursive-placeholder tests stay green.)
- `cargo test`, clippy, and the Miri slate stay green — the change touches the
  submission/placeholder path, which has frame/scope-handle memory-safety surface.

**Directions.**

- *Binder-install relocates to a dispatch-layer submit chokepoint, not a localized
  block-execution hoist — decided.* Binders can appear as arbitrary nested
  subexpressions, so the check must ride the universal "submit a dispatch" path.
  That path becomes one dispatch-layer function backed by a generic scheduler
  `alloc` primitive; it is also the natural home for the pre-sub recursion.
- *Defer binder-install to first poll (birth closure) — rejected.* Submission and
  polling are separate phases; deferring placeholder install to poll time means a
  sibling polled before the binder's slot no longer finds the placeholder, breaking
  the forward-reference invariant (execution-model.md:508-511). Install stays
  submit-time, which the chokepoint relocation preserves by construction.
- *One-shot decide closure is safe — decided (verified).* `pre_subs` is read at
  most once per slot: a poll moves the `Dispatch` work out via `take_for_run`
  (`node_store.rs:147-152`); every `Dispatch` re-install uses
  `NodeWork::dispatch(...)` with empty `pre_subs` (`nodes.rs`); keyworded parks
  re-enter through `run_dispatch_resume` with the `pre_subs` Vec *moved into* the
  captured closure, never re-read through `run_dispatch` (`execute.rs:41-42`,
  `keyworded.rs` park sites). So a birth closure that does its own pre-submission
  cannot double-run it.
- *Shape of the scheduler primitive boundary — open.* The scheduler must expose
  slot allocation that carries an opaque decide closure and the placement variants
  the current `add_dispatch_*` family encodes (run-scope `Anchored` vs per-call
  `Yoked`/in-frame). Options: (a) one `alloc_decide(run, placement, chain)` taking
  a placement enum, or (b) keep the existing `add_dispatch_*` method names as thin
  shims that build the closure via `submit_dispatch` and call a private allocator.
  Recommended: (a) — a single placement-parameterized primitive keeps the closure
  the only thing the scheduler stores and drops the per-variant method sprawl.
- *Fold `Catch` into `Combine` — deferred.* A single-dep, no-short-circuit
  `Combine` whose finish takes the raw `Result`; smaller payoff, separable from the
  AST-removal goal. Punt to a follow-up if it reads cleanly after the merge.

## Dependencies

Builds on the shipped unified scheduler interface (one `SchedulerView` in / one
`Outcome` out / `apply_outcome` the sole writer) and the `Combine`/`DispatchCombine`
merge. Update
[design/execution-model.md](../../design/execution-model.md) (the birth/resume split
and the submission-time binder-install sections) and the `nodes.rs` variant docs as
it lands.

**Requires:** none — builds on shipped scheduler substrate.

**Unblocks:** none tracked yet.
