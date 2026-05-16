# Reduce module-graph coupling and shrink AI context cost

**Problem.** Two forms of bloat hold koan back. *Structural:* the
dominant module-graph knot lives inside `koan::machine` — `machine::model`
(ast, types, values) reaches downward into `machine::core` (Scope,
KFunction, Arena handles) for runtime hooks the data layer shouldn't
depend on, contributing the bulk of `koan::machine`'s 97 cross-edges
and feeding the 217 cross-edges at the crate root between `builtins`,
`machine`, and `parse`. The crate-wide per-loc score (γ=50) is 407.69,
of which 313.79 is coupling. *Context-cost:* roughly a dozen non-test
files exceed 200 measured LOC (the modgraph size pivot), and a handful
of structurally-defensive `unreachable!` arms encode multi-step
extractor protocols the type system could fold into single calls.

**Impact.**

- *Module-graph coupling drops where it matters.* Reshuffles that break
  the `machine::model` ↓ `machine::core` back-edges pull the
  loc-normalized fractal score meaningfully below today's baseline of
  407.69.
- *AI context cost falls per file touched.* Each oversized file becomes
  several focused submodules a reader (human or model) can load
  independently; common edits stop dragging in 600–1100 lines of
  unrelated context.
- *Boilerplate stops paying compound interest.* Each new builtin body
  costs a few lines of mechanical extraction rather than 8–12 lines of
  open-coded match arms repeated across the crate.
- *Scheduler tests become a faithful surface description.* With
  type-impossible assertions deleted, each remaining test names a
  behavior worth preserving; the file reads as the scheduler's
  behavioral contract rather than a defensive shell around a previous
  representation.

**Directions.**

- *Fold peek-then-take protocol into one extractor — open.* Six
  `unreachable!("get(X) then extract_X must succeed")` arms in
  [fn_def.rs](../src/builtins/fn_def.rs) and
  [newtype_def.rs](../src/builtins/newtype_def.rs) encode a
  two-step protocol the caller cannot get wrong by construction (peek
  the variant, then take it). A combined `take_X_or_error` returning
  `Result<X, KError>` directly removes all six.
- *Split [`type_ops.rs`](../src/builtins/type_ops.rs) (345 measured
  LOC across 6 independent builtins) — open.* One submodule per body
  under `type_ops/` is the obvious partition. Scored at γ=50: net −0.36
  per-loc on the koan-wide modgraph, the only standalone file-split
  candidate that clears rounding noise on the current tree.
- *Split [`scope.rs`](../src/machine/core/scope.rs) (419
  measured LOC, dominated by one ~290-loc `impl Scope` block),
  [`fn_def.rs`](../src/builtins/fn_def.rs) (417 own-loc with a single
  `signature` child — a thin 1-child wrapper paying amplified β·scale),
  and [`kfunction.rs`](../src/machine/core/kfunction.rs) (258 own-loc,
  also a wrapper) — open.* Standalone scoring at γ=50 says scope.rs's
  natural 3-way split loses by +0.36 per-loc (the cohesive impl block
  stays oversized after any clean partition); fn_def's wrapper situation
  is the more promising target since splitting its mod.rs into siblings
  of `signature` both shrinks the largest leaf and drops the
  thin-wrapper β·scale=3 penalty. Re-score under
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
  noise off today's baseline (407.69 at γ=50). The `core` ↔ `kfunction`
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
