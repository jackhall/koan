# Execution model: graph-based, dispatch separated from execution

Koan's runtime is a three-stage pipeline. Each top-level expression flows through:

```
source ──▶ parse ──▶ dispatch ──▶ execute
        KExpression   KFuture      KObject
```

Dispatch and execution are deliberately separate stages. **Dispatch** does
name-resolution and signature-matching: given a `KExpression` and a `Scope`, it
returns a [`KFuture`](../src/dispatch/scope.rs) — the resolved `&KFunction` plus
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
Both deferring builtins (`if_then`, `KFunction::invoke` for user-fns) are tail
by construction. A chain of tail calls (`A → B → PRINT`, or unbounded
`LOOP → LOOP`) reuses one slot end-to-end. Verified by two slot-count assertions
in the test suite.

A subtle point: host-stack overflow on naïve recursion is solved by the graph
model itself, not by `Tail`. Every "recursive call" enters the FIFO queue rather
than growing the Rust call stack — that property is structural, not optimizing.
What `Tail` adds is constant **scheduler-vec** memory across the tail-call
chain.

## Open work

- **Transient-node reclamation**
  ([roadmap/transient-node-reclamation.md](../roadmap/transient-node-reclamation.md)).
  `Tail` covers the outermost frame, but body-internal sub-expressions — the
  predicate of an `IF`-guarded base case, the argument expressions to a
  recursive call — still allocate sub-`Dispatch` + `Bind` nodes per iteration,
  and those nodes are never reclaimed. Realistic recursive patterns (factorial,
  list walk) run in O(n) scheduler memory until this lands.
- **Monadic side-effect capture**
  ([roadmap/monadic-side-effects.md](../roadmap/monadic-side-effects.md)).
  `Scope::out` is one ad-hoc effect channel today; future effects (IO, time,
  randomness) need a uniform carrier that threads through the same node graph.
