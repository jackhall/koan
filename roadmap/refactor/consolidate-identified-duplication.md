# Consolidate identified code duplication

The last open cluster from a targeted duplication pass. Five of the six originally
located clusters (per-builtin `binder_name`, FN/FUNCTOR bodies, `finish.rs`
`run_combine`/`run_catch` arms, `dict_literal` accept pair, the slot-extract error
envelope) are collapsed to single owners; this item now tracks only the scheduler
`Object`/`Type` finalize-arm cluster, which is deferred behind the carrier work.

**Problem.** In [`scheduler/execute.rs`](../../src/machine/execute/scheduler/execute.rs)
the `Carried::Object` arm and the `Carried::Type` arm run near-identical declared-return
extraction, `matches_value` mismatch check, and re-tag — the `Type` arm's own comment
names itself "the type-channel analog of the `Object` arm above." The duplication is
the surface symptom of the `Carried::Type` / `Carried::Object` fork that
[type values as data carriers](../type_language/type-values-as-data-carriers.md)
dissolves.

**Acceptance criteria.**

- The scheduler's declared-return check exists once, parameterized over the lifted
  carrier, not duplicated across the `Object` and `Type` arms.

**Directions.**

- *Scheduler-arm consolidation vs. carrier unification — deferred.* Defer this cluster
  behind [type values as data carriers](../type_language/type-values-as-data-carriers.md):
  that item removes the `Carried::Type` / `Carried::Object` fork the arm duplication is a
  symptom of, so consolidating the arms first would be rework. Consolidate after the fork
  dissolves, or let the carrier work dissolve both arms together.

## Dependencies

Overlaps the [naming-and-responsibility audit](naming-and-responsibility-audit.md), whose
"duplicated responsibility" category would surface this same cluster.

**Requires:** [type values as data carriers](../type_language/type-values-as-data-carriers.md) — dissolves the carrier fork the arm duplication is a symptom of.

**Unblocks:** none tracked yet.
