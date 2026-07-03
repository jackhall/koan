# Execution model: graph-based, dispatch separated from execution

Koan's runtime is a three-stage pipeline. Each top-level expression flows through:

```
source ──▶ parse ──▶ dispatch ──▶ execute
        KExpression  ResolveOutcome  KObject
```

Dispatch and execution are deliberately separate stages. **Dispatch** does
name-resolution and signature-matching: given a `KExpression` and a `Scope`, it
returns a [`ResolveOutcome`](../../src/machine/execute/dispatch/resolve_dispatch.rs) — on a
unique match, the resolved `&KFunction` with its bound arguments, ready to run but not yet
executed. **Execution** is what
the [`Scheduler`](../../src/machine/execute/run_loop.rs) does: it owns a DAG of deferred
work, decides when each resolved call runs, and hands its body the live scope.


## The model, in three parts

- [The scheduler runtime](scheduler.md) — dispatch as scheduler nodes, the
  decide→outcome→apply boundary, dependency edges and their invariants, the
  splices, tail-call rewriting, transient-node reclamation, and the build-vs-run
  execution modes.
- [Name placeholders and submission](name-placeholders.md) — forward-reference
  name placeholders and submission-time binder install (the submit side).
- [Classify and apply](classify-and-apply.md) — the shape classifier, the fast
  lanes, the keyworded apply pipeline, and dispatch birth/resume (the execute side).
- [Calls, values, and performance](calls-and-values.md) — the `KObject` model/core
  boundary, performance characteristics, the lexical provenance chain, and open work.
