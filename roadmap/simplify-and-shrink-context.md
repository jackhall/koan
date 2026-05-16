# Simplify runtime::machine and shrink AI context cost

**Problem.** Two forms of bloat hold koan back. *Structural:* at HEAD
(`0e38fab`) the dominant knot has moved up a level from `runtime::machine`
into `runtime` itself — `koan::runtime` scores cross=220 / feedback=21 /
index=262 and accounts for ~69% of the crate-wide Σ index·loc (2,011,270;
loc-normalized 88.46). 18 of the 21 back-edges run `model → machine`: the
data layer reaches into runtime machinery instead of sitting under it.
Inside `machine` itself the score has come down (cross=52 / fb=8 / index=68
after the Bindings / PendingQueue / scope-test-split work), with a residual
`core → kfunction` tangle (8 back-edges). *Context-cost:* three non-test
files still exceed 550 lines —
[interpret.rs](../src/runtime/machine/execute/interpret.rs) (619),
[ascribe.rs](../src/runtime/builtins/ascribe.rs) (614),
[kfunction.rs](../src/runtime/machine/core/kfunction.rs) (561). The scheduler's
two test files
([tests.rs](../src/runtime/machine/execute/scheduler/tests.rs) 486 lines,
[run_tests.rs](../src/runtime/machine/execute/scheduler/run_tests.rs) 244
lines) still carry assertions that the recent DepGraph and NodeStore
sub-struct extractions made type-impossible.

**Impact.**

- *Module-graph coupling drops where it matters.* A reshuffle that breaks
  more than one of the remaining `runtime::machine` back-edges, plus
  LOC-redistribution from splitting the three 600+-line files, pulls the
  loc-normalized fractal score meaningfully below the 87.87 baseline.
- *AI context cost falls per file touched.* Each of the three 600+-line files
  becomes 2–3 focused submodules a reader (human or model) can load
  independently; common edits stop dragging in 600 lines of unrelated context.
- *Scheduler tests become a faithful surface description.* With type-impossible
  assertions deleted, each remaining test names a behavior worth preserving;
  the file reads as the scheduler's behavioral contract rather than a
  defensive shell around the old vector-of-vectors representation.
- *Stage-2 scheduler work lands in a cleaner layout.* The substrate the
  module-system stage-2 follow-on rides on is the same `runtime::machine`
  subtree being reshuffled here, so doing the reshuffle first saves redoing
  layout work mid-stage.

**Directions.**

- *Break the `model → machine` back-edges — open.* The dominant knot is no
  longer inside `runtime::machine` (down to cross=52 / fb=8 / index=68); it is
  one level up, where `runtime::model` reaches into `runtime::machine` 18
  times. Those 18 back-edges drive most of `runtime`'s index=262 and
  `runtime::model` accounts for ~69% of the crate-wide Σ index·loc at HEAD
  (2,011,270; loc-normalized 88.46). Identify what `model::types` and
  `model::values` reach into `machine` for (Scope, KFunction, Arena handles,
  scheduler types) and either (a) lift the offending APIs out of `model` into
  `machine`, or (b) demote the shared substrate from `machine` to a sibling
  of `model` so the dependency points downward. Score candidates with
  [tools/modgraph_rewrite.py](../tools/modgraph_rewrite.py) plus `modgraph.py
  --fractal koan`; adopt only reshuffles whose loc-normalized score drops by
  more than rounding noise off the 88.46 baseline. The smaller `core →
  kfunction` tangle inside `machine` (8 back-edges, ~8% of total) is a
  secondary target — bundle it into the same reshuffle only if a single
  partition addresses both.
- *Split the three remaining 550+-line files — open.* Each is plausibly 2–3
  focused submodules, but the right split for `interpret.rs` / `ascribe.rs`
  / `kfunction.rs` depends on which partition wins above. Score candidate
  splits in the same `modgraph_rewrite.py` pass so LOC redistribution and
  edge re-classification are evaluated together.
- *Trim scheduler tests against the new sub-struct surface — decided.* Delete
  tests asserting tri-vector shape invariants that `DepGraph` / `NodeStore`
  now type-enforce; keep behavior-level tests (scheduling order, completion,
  error propagation). Do this *after* the structural reshuffle so the trim
  isn't redone against a moved scheduler.
- *Crate-wide `/trim-comments` pass — decided.* Run once after structural
  moves settle, so the diff doesn't tangle with file moves.

## Dependencies

**Requires:** none — all four directions run against shipped infrastructure
(DepGraph / NodeStore sub-structs from `8f637d1` / `1ebcccd`, scheduler
simplification from `273721f`, and the existing modgraph tooling).

**Unblocks:** none.
