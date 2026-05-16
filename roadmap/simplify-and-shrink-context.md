# Simplify runtime::machine and shrink AI context cost

**Problem.** Two forms of bloat hold koan back. *Structural:* at HEAD the
dominant module-graph knot has migrated up a level from `runtime::machine`
into `runtime` itself; `koan::runtime` accounts for roughly two-thirds of the
crate-wide Σ index·loc, driven by back-edges from `model` into `machine`
(the data layer reaching into runtime machinery instead of sitting under
it). *Context-cost:* ten non-test files exceed 550 lines, and a handful
of structurally-defensive `unreachable!` arms encode multi-step
extractor protocols the type system could fold into single calls.

**Impact.**

- *Module-graph coupling drops where it matters.* Reshuffles that break
  `model → machine` back-edges pull the loc-normalized fractal score
  meaningfully below today's baseline.
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
  [fn_def.rs](../src/runtime/builtins/fn_def.rs) and
  [newtype_def.rs](../src/runtime/builtins/newtype_def.rs) encode a
  two-step protocol the caller cannot get wrong by construction (peek
  the variant, then take it). A combined `take_X_or_error` returning
  `Result<X, KError>` directly removes all six.
- *Split [`type_ops.rs`](../src/runtime/builtins/type_ops.rs) (1102
  LOC) — open.* Six independent builtin bodies sharing one trivial
  helper; one submodule per body under `type_ops/` is the obvious
  partition. Largest file in the crate; clean isolation.
- *Split
  [`ktype_predicates.rs`](../src/runtime/machine/model/types/ktype_predicates.rs)
  (698 LOC) — open.* Three disjoint concerns under one module:
  specificity ordering, per-`ExpressionPart` admissibility, per-value
  type-tagging. Each is independent of the others.
- *Split [`ascribe.rs`](../src/runtime/builtins/ascribe.rs) (800
  LOC) — open.* The two body functions (`body_opaque`,
  `body_transparent`) plus shape-checking and abstract-type-name
  sweeping; partition along those seams.
- *Split [`bindings.rs`](../src/runtime/machine/core/bindings.rs) (855
  LOC) — open.* Single façade `impl` with high cohesion but identifiable
  bands: data/functions write primitives, types write primitive,
  transactional nominal dual-writes, and the `PendingBinderGuard` RAII
  machinery. Lower partition return than the others above; bundle with
  a `machine` reshuffle pass rather than as a standalone PR.
- *Split [`scope.rs`](../src/runtime/machine/core/scope.rs) (742 LOC)
  and the remaining 600+-line builtins
  ([`fn_def.rs`](../src/runtime/builtins/fn_def.rs),
  [`struct_def.rs`](../src/runtime/builtins/struct_def.rs),
  [`let_binding.rs`](../src/runtime/builtins/let_binding.rs),
  [`interpret.rs`](../src/runtime/machine/execute/interpret.rs),
  [`kfunction.rs`](../src/runtime/machine/core/kfunction.rs)) — open.*
  Each is plausibly 2–3 focused submodules but the right seams depend
  on the structural reshuffle below. Score candidate splits in the same
  [`modgraph_rewrite.py`](../tools/modgraph_rewrite.py) pass so LOC
  redistribution and edge re-classification are evaluated together.
- *Break the `model → machine` back-edges — open.* The dominant knot
  has moved up a level; `runtime::model` reaches into `runtime::machine`
  enough times to drive most of the crate-wide Σ index·loc. Identify
  what `model::types` and `model::values` reach into `machine` for
  (Scope, KFunction, Arena handles, scheduler types) and either lift
  the offending APIs out of `model` into `machine`, or demote the
  shared substrate from `machine` to a sibling of `model` so the
  dependency points downward. Score candidates with
  [`modgraph_rewrite.py`](../tools/modgraph_rewrite.py); adopt only
  reshuffles whose loc-normalized score drops by more than rounding
  noise off today's baseline. The smaller `core → kfunction` tangle
  inside `machine` is a secondary target — bundle only if a single
  partition addresses both.
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
