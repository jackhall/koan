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

For asynchronous chains (a body whose result depends on a deferred computation)
the result vec carries `Forward(NodeId)`, deferring cleanly until the dependency
resolves.

## Tail-call optimization

[`BodyResult::Tail(KExpression)`](../src/dispatch/kfunction.rs) makes a tail
return rewrite the **current scheduler slot's work** to a fresh
`Dispatch(expr)` and re-run in place. No new node allocated, no `Forward` chain.
Both deferring builtins (`match_case`, `KFunction::invoke` for user-fns) are
tail by construction. A chain of tail calls (`A → B → PRINT`, or unbounded
`LOOP → LOOP`) reuses one slot end-to-end. Verified by two slot-count assertions
in the test suite.

A subtle point: host-stack overflow on naïve recursion is solved by the graph
model itself, not by `Tail`. Every "recursive call" enters the FIFO queue rather
than growing the Rust call stack — that property is structural, not optimizing.
What `Tail` adds is constant **scheduler-vec** memory across the tail-call
chain.

## Transient-node reclamation

`Tail` reuses the outermost slot but bodies typically have internal
sub-expressions — the predicate of an `IF`/`MATCH` guard, the argument
expressions of a recursive call, list/dict literal elements. Each spawns a
sub-`Dispatch` and a parent `Bind`/`Aggregate` slot. Without reclamation those
slots accumulate per body iteration, so realistic recursive code is O(n)
scheduler memory even when its data footprint is O(1).

Reclamation runs in two places:

- **Eager free at end of `run_bind` / `run_aggregate*`.** Once a Bind has read
  its dep results and spliced them into `expr.parts` as `Future(value)` (or an
  Aggregate has deep-cloned each element into the materialized list/dict), the
  dep slots are unreachable — Forward chains never target sub-`Dispatch`
  slots, and a sub-Dispatch is read by exactly one Bind/Aggregate. Free walks
  recursively, recycling each dep's own forward chain and dep tree.
- **Chain-free at finalize.** When `finalize_ready_frames` collapses a
  frame-holder's `Forward(target)` into the lifted `Value`, the chain target
  is freed. Free's `nodes[i].is_some()` guard makes the recursion stop at any
  still-live slot — frame holders, queued Binds — so each in-flight user-fn
  call handles its own subtree at its own finalize.

The net effect: recursive bodies whose only persistent state is the call
result run in O(1) scheduler memory across iterations, with the per-iteration
fanout (the body's transient sub-Dispatches/Binds) recycled through a
free-list of slot indices that `add()` pulls from before extending the vecs.
Bookkeeping lives in two `Scheduler` sidecars: `node_dependencies:
Vec<Vec<usize>>` (each Bind/Aggregate slot's owned sub-slot indices, captured
at `add()` time before `take()` consumes the work) and `free_list:
Vec<usize>`. See also [memory-model.md § Performance notes](memory-model.md).

A known limitation: each top-level dispatch retains two persistent slots —
the entry slot returned to the user and the Bind it forwards to (which the
user-fn body's lift writes its terminal `Value` into). Top-level forward
chains have no finalize trigger that would collapse them into a direct
`Value`, so each `add_dispatch` costs a small constant rather than one slot.
Linear in call count, not multiplicative in body size; closing it would need
either path compression in `read_result` or a post-execute pass.

## Open work

- **Compile-time scheduling for the type checker**
  ([module-system.md § Compile-time scheduling](module-system.md#compile-time-scheduling)).
  The [module system](module-system.md) adds two new node types alongside
  `Dispatch` — `Infer(expr, ctx)` for type inference and
  `ImplicitSearch(sig, types, scope)` for modular-implicit resolution.
  Inference and search interleave (search needs constrained types resolved;
  resolved implicits refine type variables that other inferences are waiting
  on), and the existing scheduler's dependency tracking and cycle detection
  carry both. Stage 1
  ([roadmap/module-system-1-module-language.md](../roadmap/module-system-1-module-language.md))
  is where this lands; the scheduler grows multi-target unification and a
  type-check-vs-evaluation phase boundary at the same time.
- **Monadic side-effect capture**
  ([roadmap/monadic-side-effects.md](../roadmap/monadic-side-effects.md)).
  `Scope::out` is one ad-hoc effect channel today; future effects (IO, time,
  randomness) need a uniform carrier that threads through the same node graph.
