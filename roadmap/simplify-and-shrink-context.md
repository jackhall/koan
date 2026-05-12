# Simplify runtime::machine and shrink AI context cost

**Problem.** Two forms of bloat hold koan back. *Structural:* the
`runtime::machine` subtree dominates the module-graph fractal index — at HEAD
(`273721f`) `koan::runtime` scores cross=220 / feedback=21 / index=262 and the
`machine` child alone is cross=48 / fb=6 / index=60, roughly 60% of the
crate-wide Σ index·loc (1,935,797; loc-normalized 87.87). Six back-edges run
between `machine::core`, `machine::execute`, and `machine::kfunction`.
*Context-cost:* four non-test files exceed 600 lines each —
[dispatcher.rs](../src/runtime/machine/core/dispatcher.rs) (663),
[scope.rs](../src/runtime/machine/core/scope.rs) (645),
[ascribe.rs](../src/runtime/builtins/ascribe.rs) (625),
[interpret.rs](../src/runtime/machine/execute/interpret.rs) (619). The
scheduler's two test files
([tests.rs](../src/runtime/machine/execute/scheduler/tests.rs) 382 lines / 12
tests, [run_tests.rs](../src/runtime/machine/execute/scheduler/run_tests.rs)
244 lines / 15 tests) still carry assertions that the recent DepGraph and
NodeStore sub-struct extractions made type-impossible.

**Impact.**

- *Module-graph coupling drops where it matters.* A reshuffle that breaks even
  three of the six `runtime::machine` back-edges, plus LOC-redistribution from
  splitting the four 600+-line files, pulls the loc-normalized fractal score
  meaningfully below the 87.87 baseline.
- *AI context cost falls per file touched.* Each of the four 600+-line files
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

- *Structural reshuffle of `runtime::machine` — open.* Candidate partitions
  (pulling `dispatcher` out of `core`, merging `kfunction` into `execute`,
  hoisting shared types up to `machine`) score via
  [tools/modgraph_rewrite.py](../tools/modgraph_rewrite.py) plus
  `modgraph.py --fractal koan` against the rewritten DOT. Adopt only
  reshuffles whose loc-normalized score drops by more than rounding noise off
  the 87.87 baseline; otherwise the file moves aren't paying for themselves.
- *Split the four 600+-line files — open.* Each is plausibly 2–3 focused
  submodules, but the right split for `dispatcher.rs` / `scope.rs` /
  `ascribe.rs` / `interpret.rs` depends on which partition wins above. Score
  candidate splits in the same `modgraph_rewrite.py` pass so LOC
  redistribution and edge re-classification are evaluated together.
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

**Unblocks:** none strictly — but
[module-system-2-scheduler](module-system-2-scheduler.md) is the natural
follow-on, and doing the partition exercise first means stage-2 work lands in
the new layout instead of moving with it.
