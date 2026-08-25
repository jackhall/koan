# Execution model: graph-based, dispatch separated from execution

Koan's runtime is a three-stage pipeline. Each top-level expression flows through:

```
source ──▶ parse ──▶ dispatch ──▶ execute
        KExpression  DispatchOutcome  KObject
```

Dispatch and execution are deliberately separate stages. **Dispatch** does
name-resolution and signature-matching: given a `KExpression` and a `Scope`, it
returns a [`DispatchOutcome`](../../src/machine/execute/decide/resolve_dispatch.rs) — on a
unique match, the resolved `&KFunction` with its bound arguments, ready to run but not yet
executed. **Execution** is what
the [`Scheduler`](../../workgraph/src/scheduler.rs) does: it owns a DAG of deferred
work, decides when each resolved call runs, and hands its body the live scope.


## The model, in three parts

- [The scheduler runtime](scheduler.md) — dispatch as scheduler nodes, the
  decide→outcome→apply boundary, dependency edges and their invariants, the
  splices, tail-call rewriting, transient-node reclamation, and the build-vs-run
  execution modes.
- [Name placeholders and submission](name-placeholders.md) — forward-reference
  name placeholders and submission-time binder install (the submit side).
- [The continuation currency](continuations.md) — the one stored-continuation
  signature, generic combinator composition with a single erasure, the two-tier
  bumped/boxed erase door, and the co-location and capture rules.
- [Classify and apply](classify-and-apply.md) — the shape classifier, the fast
  lanes, the keyworded apply pipeline, and dispatch birth/resume (the execute side).
- [Calls, values, and performance](calls-and-values.md) — the `KObject` model/core
  boundary, performance characteristics, the lexical provenance chain, and open work.
- [Value equality](value-equality.md) — the `==` / `!=` builtins, the structural
  `value_equal` walk, the comparability gate and its deliberate intransitivity, the
  function/module ban with the `TYPE OF` idiom, and dict-key normalization.
