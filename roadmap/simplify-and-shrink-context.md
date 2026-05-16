# Reduce module-graph coupling and shrink AI context cost

**Problem.** Two forms of bloat hold koan back. *Structural:* the
dominant module-graph knot lives inside `koan::machine` — `machine::model`
(ast, types, values) reaches downward into `machine::core` (Scope,
KFunction, Arena handles) for runtime hooks the data layer shouldn't
depend on, contributing the bulk of `koan::machine`'s cross-edges and
feeding the cross-edges at the crate root between `builtins`,
`machine`, and `parse`. The crate-wide per-loc score (γ=50) is 245.88,
of which 63.10 is coupling and 120.14 is per-file size. *Context-cost:*
several non-test files still exceed 200 raw LOC (the size pivot) —
`bindings.rs` at 809 raw, `ascribe.rs` at 751 raw, `scope.rs` at 702
raw, `fn_def.rs` at 662 raw — each carrying a 5-figure size charge.

**Impact.**

- *Module-graph coupling drops where it matters.* Reshuffles that break
  the `machine::model` ↓ `machine::core` back-edges pull the
  loc-normalized fractal score meaningfully below today's baseline of
  245.88.
- *AI context cost falls per file touched.* Each oversized file becomes
  several focused submodules a reader (human or model) can load
  independently; common edits stop dragging in 600–1000 lines of
  unrelated context.
- *Scheduler tests become a faithful surface description.* With
  type-impossible assertions deleted, each remaining test names a
  behavior worth preserving; the file reads as the scheduler's
  behavioral contract rather than a defensive shell around a previous
  representation.

**Directions.**

- *Split [`scope.rs`](../src/machine/core/scope.rs) (419 prod / 702
  raw LOC, dominated by one ~290-loc `impl Scope` block),
  [`fn_def.rs`](../src/builtins/fn_def.rs) (417 prod / 662 raw with a
  single `signature` child — a thin 1-child wrapper paying amplified
  β·scale), and [`kfunction.rs`](../src/machine/core/kfunction.rs) (258
  prod / 574 raw, also a wrapper) — open.* The earlier-scored +0.36
  per-loc loss for scope.rs's natural 3-way split predates the
  raw-LOC size term and edge-dedup coupling fix — re-score against
  the current 245.88 baseline before adopting. `fn_def`'s wrapper
  situation is the more promising target since splitting its `.rs`
  into siblings of `signature` both shrinks the largest leaf and
  drops the thin-wrapper β·scale=3 penalty. Re-score under
  [`modgraph_rewrite.py`](../tools/modgraph_rewrite.py) bundled with
  the structural reshuffle below — LOC redistribution and edge
  re-classification need joint evaluation.
- *Break the `machine::model` ↓ `machine::core` back-edges — open.*
  `machine::model::types` and `machine::model::values` reach
  sideways/downward into `machine::core` (Scope, KFunction, Arena
  handles) and `machine::execute` (scheduler types) enough times to
  drive most of `machine`'s 97 cross-edges. Either lift the offending
  APIs out of `model` into `core`/`execute`, or promote the shared
  substrate (e.g. `ast`, `types`) out of `model` to a sibling of
  `machine` so dependencies point one way. Score candidates with
  [`modgraph_rewrite.py`](../tools/modgraph_rewrite.py); adopt only
  reshuffles whose loc-normalized score drops by more than rounding
  noise off today's baseline (245.88 at γ=50). The `core` ↔ `kfunction`
  tangle inside `machine` is a secondary target — bundle only if a
  single partition addresses both.
- *Trim scheduler tests against the sub-struct surface — decided.*
  Delete tests asserting tri-vector shape invariants that `DepGraph` /
  `NodeStore` now type-enforce; keep behavior-level tests (scheduling
  order, completion, error propagation). Run after the structural
  reshuffle so the trim isn't redone against a moved scheduler.
- *Crate-wide `/trim-comments` pass — decided.* Run once after
  structural moves settle, so the diff doesn't tangle with file moves.

## Dependencies

**Requires:** none — every direction runs against shipped infrastructure.

**Unblocks:** none.
