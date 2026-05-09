# Execution model: graph-based, dispatch separated from execution

Koan's runtime is a three-stage pipeline. Each top-level expression flows through:

```
source ──▶ parse ──▶ dispatch ──▶ execute
        KExpression   KFuture      KObject
```

Dispatch and execution are deliberately separate stages. **Dispatch** does
name-resolution and signature-matching: given a `KExpression` and a `Scope`, it
returns a [`KFuture`](../src/dispatch/runtime/scope.rs) — the resolved `&KFunction` plus
its `ArgumentBundle`, ready to run but not yet executed. **Execution** is what
the [`Scheduler`](../src/execute/scheduler.rs) does: it owns a DAG of deferred
work, decides when each `KFuture` runs, and hands its body the live scope.

## Dispatch as a scheduler node

The scheduler models dispatch itself as a node type — `Dispatch(KExpression)`.
[`schedule_expr`](../src/execute/interpret.rs) collapses to "add a `Dispatch`
node per top-level expression"; the rest is dynamic. At run time a `Dispatch`
walks its expression's parts, spawns sub-`Dispatch`/`Bind`/`Aggregate` nodes for
nested sub-expressions, and a builtin body holding `&mut dyn SchedulerHandle`
can also add `Dispatch` nodes.

## `BodyResult` — the three return shapes

A builtin body returns one of:

```rust
BodyResult { Value(&KObject) | Tail(KExpression) | Err(KError) }
```

- `Value` — the body produced a final value; the slot finalizes.
- `Tail` — the body wants to dispatch a fresh expression in its own slot (TCO,
  see below).
- `Err` — structured failure; see [error-handling.md](error-handling.md).

When a body cannot produce its result inline — its expression has nested
sub-expressions whose own evaluation hasn't run yet — the slot's work is
rewritten to `Lift { from: NodeId }` (a [`NodeWork`](../src/execute/nodes.rs)
variant). The Lift shim parks on the spawned `Bind`'s notify-list, waits
for that slot's terminal write, and copies the result into its own slot when
it runs. The original slot keeps its frame and notify-list across the
rewrite, so consumers downstream see the eventual terminal as if the body
had produced it directly.

## Push/notify dependency edges

The scheduler's edges point producer → consumer. Each slot carries a
`notify_list: Vec<NodeId>` of dependents waiting on it; each `Bind` /
`Aggregate` / `Lift` consumer carries a `pending_deps: usize` counter of
unresolved deps. When a slot writes a terminal `Value` or `Err`, the
notify-walk drains its `notify_list`, decrements each consumer's
`pending_deps`, and pushes any zero-counter consumer onto the run-set
([`Scheduler::notify_consumers`](../src/execute/scheduler.rs)). Consumers
arrive on the run-set only when actually ready; there is no poll-and-requeue.

The run-set has two priority bands. Internal work goes through `ready_set`
(populated by the notify-walk and by ready-on-arrival nodes registered in
`add()`). Top-level `add_dispatch` calls go through a separate FIFO `queue`
so independent top-level expressions execute in submission order. The
execute loop drains `ready_set` first, then `queue`.

## Tail-call optimization

[`BodyResult::Tail(KExpression)`](../src/dispatch/kfunction.rs) makes a tail
return rewrite the **current scheduler slot's work** to a fresh
`Dispatch(expr)` and re-run in place — no new node allocated. Both deferring
builtins (`match_case`, `KFunction::invoke` for user-fns) are tail by
construction. A chain of tail calls (`A → B → PRINT`, or unbounded
`LOOP → LOOP`) reuses one slot end-to-end. Verified by two slot-count
assertions in the test suite.

A subtle point: host-stack overflow on naïve recursion is solved by the graph
model itself, not by `Tail`. Every "recursive call" enters the scheduler's
run-set rather than growing the Rust call stack — that property is
structural, not optimizing. What `Tail` adds is constant **scheduler-vec**
memory across the tail-call chain.

## Transient-node reclamation

`Tail` reuses the outermost slot but bodies typically have internal
sub-expressions — the predicate of an `IF`/`MATCH` guard, the argument
expressions of a recursive call, list/dict literal elements. Each spawns a
sub-`Dispatch` and a parent `Bind`/`Aggregate` slot. Without reclamation those
slots accumulate per body iteration, so realistic recursive code is O(n)
scheduler memory even when its data footprint is O(1).

Reclamation runs at the end of `run_bind` / `run_aggregate*`. Once a Bind has
read its dep results and spliced them into `expr.parts` as `Future(value)`
(or an Aggregate has deep-cloned each element into the materialized
list/dict), the dep slots are unreachable: a sub-Dispatch is owned by exactly
one Bind / Aggregate, recorded in the consumer's `node_dependencies` entry.
Free walks recursively, recycling each dep's own dep tree, and stops at any
still-live slot via `nodes[i].is_some()` — so a free that dives into another
in-flight user-fn call leaves that subtree for that call's own reclamation.

The net effect: recursive bodies whose only persistent state is the call
result run in O(1) scheduler memory across iterations, with the per-iteration
fanout (the body's transient sub-Dispatches/Binds) recycled through a
free-list of slot indices that `add()` pulls from before extending the vecs.
Bookkeeping lives in three `Scheduler` sidecars: `notify_list:
Vec<Vec<NodeId>>` (each producer's dependent list), `pending_deps: Vec<usize>`
(each consumer's unresolved-dep counter), and `node_dependencies:
Vec<Vec<usize>>` (each Bind/Aggregate slot's owned sub-slot indices, captured
at `add()` time before `take()` consumes the work and used by `free()` to
walk the ownership tree). The `free_list: Vec<usize>` carries indices whose
`nodes`/`results`/`notify_list`/`pending_deps`/`node_dependencies` entries
are cleared and ready for reuse. See also [memory-model.md § Performance
notes](memory-model.md).

A known limitation: each top-level dispatch retains two persistent slots —
the entry `Lift` slot returned to the user, and the `Bind` it lifts from
(which the user-fn body writes its terminal `Value` into). Neither has a
parent to free it, so each `add_dispatch` costs a small constant rather than
one slot. Linear in call count, not multiplicative in body size; closing it
would need a post-execute compaction pass.

## Pegged and free execution

Koan code is built once and run many times, but build-time and run-time are
the same engine — the scheduler from this document runs both. The only
difference is that some nodes' results depend on data or effects unavailable
at build time, and those nodes are **pegged** — held without execution
until the data or effect arrives. Build-time runs the scheduler against
the full DAG; nodes that are not pegged execute (and produce values, refine
types, spawn dependents) freely; the run halts at the pegged frontier.
Run-time supplies the inputs and effects, unblocks the pegged nodes, and
the scheduler resumes — same machinery, no new pass.

- **Nodes pegged at build time:** user-supplied input; source files for
  plugins not available at build time; syscalls in builtins; network calls.
- **Nodes that execute freely at build time:** source files available at
  build time; entropy/randomness used for property-test axiom checking and
  cross-implicit equivalence checking.

The intermediate representation is the **stalled DAG state** — the
scheduler's `nodes` / `results` arrays at the free-execution fixed point,
plus the identifiers of pegged nodes. Run-time consumes that state
directly: skip parsing, supply the pegged inputs and effects, continue
running the scheduler.

There is no separate type-checking phase preceding evaluation. Inference,
dispatch, and execution interleave in one DAG; build-time is the same
engine running before pegged inputs are unblocked.

## Open work

- **Inference and search as scheduler work**
  ([module-system.md § Inference and search](module-system.md#inference-and-search-as-scheduler-work)).
  Type inference and modular-implicit resolution reduce to the existing
  `Dispatch` and `Bind` machinery — type-returning builtins on the value
  path, `Bind` as the refinement-and-wake-up mechanism, and stage 5
  implicit search as a single `SEARCH_IMPLICIT` builtin rather than a new
  node kind. Module-system
  [stage 2](../roadmap/module-system-2-scheduler.md) lands the type-builtin
  substrate end-to-end through FN signatures;
  [stage 5](../roadmap/module-system-5-modular-implicits.md) layers
  implicit search on top.
- **Monadic side-effect capture**
  ([roadmap/monadic-side-effects.md](../roadmap/monadic-side-effects.md)).
  `Scope::out` is one ad-hoc effect channel today; future effects (IO, time,
  randomness) need a uniform carrier that threads through the same node graph.
