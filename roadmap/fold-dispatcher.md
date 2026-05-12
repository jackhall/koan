# Fold the dispatcher into `Scope`, `KFunction`, and `ExpressionSignature`

**Problem.** Overload resolution is split across two files that don't carve at the
natural joints. The "core" dispatcher in
[dispatcher.rs](../src/runtime/machine/core/dispatcher.rs) (663 lines) is a pile of
free functions taking `&Scope` — `dispatch`, `lazy_candidate`, `shape_pick`,
`pick_most_specific_index`, `accepts_for_wrap`, `lazy_eager_indices`,
`classify_for_pick` — that has neither a coherent type of its own nor a single
entry point. Three of those four `Scope`-receiving entries
(`dispatch` / `lazy_candidate` / `shape_pick`) are called sequentially from the
scheduler-side driver in
[scheduler/dispatch.rs](../src/runtime/machine/execute/scheduler/dispatch.rs)
against the same `(scope, expr)` pair, each re-walking `scope.outer`, re-keying
with `expr.untyped_key()`, and re-running the specificity tournament. A fourth
walk lives in `install_dispatch_placeholder` (lines 10-41 of the same file),
which re-iterates the same bucket hunting for `pre_run`. Inside
`shape_pick`, `classify_for_pick` re-runs `lazy_eager_indices` on the picked
candidate, so the four chain walks plus the redundant classifier walk add up
to five passes for one call site. The slot-classification result is three
parallel `Vec<usize>` (`eager_indices` / `wrap_indices` / `ref_name_indices`)
documented as disjoint by comment only — the `match other => other` branch at
`scheduler/dispatch.rs:107` is a comment-enforced invariant whose violation
would be a silent classifier bug, and the `unreachable!` at line 183 is the
same shape of risk on the eager path. `Scope::dispatch` / `lazy_candidate` /
`shape_pick` have no non-test callers outside the scheduler (`finish.rs:38` is
the one secondary use); the broad surface area exists only to support
the four-walk consumption pattern.

Three semantic invariants live in today's code and must survive the refactor
unchanged — they are easy to drift on when the four walks fold into one:

- *Strict-then-tentative is per scope, not whole-chain.* `shape_pick` tries
  the strict pass first within a single scope's bucket, then the tentative
  (auto-wrap) pass within the *same* scope, and only descends to `outer` if
  both miss. A unified walk that runs strict across the whole chain before
  tentative would change semantics.
- *Ambiguity returns at the first scope where strict mode ties.* Today
  `dispatch` errors as soon as `pick_most_specific_index` reports more than
  one candidate at a given scope; it does not fall through to `outer` hoping
  for a clearer pick. The new `ResolveOutcome::Ambiguous(usize)` fires at the
  same point.
- *Per-slot classification is fixed by the `(SignatureElement, ExpressionPart)`
  pair, not by global state.* The three role buckets carry today's exact
  rules: `wrap_indices` holds slots where `accepts_for_wrap` succeeded via
  the auto-wrap rule (`SignatureElement::Type` accepting an
  `ExpressionPart::Identifier` etc.); `ref_name_indices` holds slots where
  `classify_for_pick` flagged a bare-name reference into a name slot
  (`SignatureElement::Name`); `eager_indices` holds the
  `ExpressionPart::Expression` slots whose signature side is *not* a
  KExpression slot (i.e. eager evaluation required). All three sets are
  disjoint by construction over disjoint `(element, part)` shapes; the
  refactor's job is to make that disjointness type-enforced, not to relax
  or extend the rules.

**Impact.**

- *Delete a 663-line file.* `dispatcher.rs` goes away. Roughly 150 lines of
  logic redistribute to natural homes: ~80 to `Scope` (the one resolution
  walk), ~40 to `KFunction` (the shape predicates that already key off
  `f.signature` and `f.pre_run`), ~30 to `ExpressionSignature` (the
  specificity tournament). The remainder was duplication.
- *One chain walk per call site.* The scheduler driver consumes a single
  `Resolved` instead of stitching three results together. The `pre_run`
  placeholder name falls out of the same walk, so
  `install_dispatch_placeholder` deletes outright.
- *Slot-role disjointness becomes type-enforced.* Either the three index
  vectors stay on a `Resolved` struct whose constructor is the only producer
  (so the disjointness invariant lives in one place), or the equivalent
  `Vec<SlotRole>`-per-slot representation lets downstream `run_dispatch` walk
  the slots in one pass instead of three.
- *Scheduler driver becomes a linear pipeline.* `run_dispatch`'s 175-line
  straight-line state machine splits into named phases that each do one
  thing — short-circuit, auto-wrap, replay-park, schedule deps — and the
  near-duplicate lazy/eager scheduling loops at lines 174-195 and 197-226
  collapse into one loop driven by `Resolved`'s role indices.
- *Layering cleanup.* `f.pre_run.is_some()` reads inside free functions go
  away once the predicates live on `KFunction`. The `core::dispatcher` →
  `core::scope` plus `core::dispatcher` → `kfunction` back-edges that
  `runtime::machine`'s fractal-index score pays for in
  [simplify-and-shrink-context.md](simplify-and-shrink-context.md) drop with
  the file deletion.

**Directions.**

- *Where each piece goes — decided.* Outer-chain walk + bucket lookup +
  strict-then-tentative ordering becomes one `Scope::resolve(expr) ->
  ResolveOutcome` method, the sole resolution entry point. Shape predicates
  (`lazy_eager_indices`, `accepts_for_wrap`, `classify_for_pick`) become
  methods on [kfunction.rs](../src/runtime/machine/kfunction.rs) — they key
  off `f.signature.elements` and `f.pre_run` while the `KExpression` side
  only contributes per-`parts[i]` matching, and placing them on `KExpression`
  would introduce a fresh ast→runtime back-edge that `kfunction.rs`'s
  existing imports of `ExpressionPart` / `KExpression` / `SignatureElement`
  let us avoid. Specificity ranking (`pick_most_specific_index`) becomes
  `ExpressionSignature::most_specific(candidates: &[&Self]) -> Option<usize>`
  in [signature.rs](../src/runtime/model/types/signature.rs); no trait — the
  codebase has exactly one ranker.
- *`Resolved` and `ResolveOutcome` shape — decided.* `Resolved` carries
  `function: &'a KFunction<'a>`, `placeholder_name: Option<String>`,
  `eager_indices` / `wrap_indices` / `ref_name_indices: Vec<usize>`, and
  `picked_has_pre_run: bool`; absorbs today's `ShapePick`.
  `ResolveOutcome` is a three-variant enum — `Resolved(Resolved<'a>)`,
  `Ambiguous(usize)`, `Unmatched` — with no separate Unmatched-but-tentative
  variant (today's code already collapses that distinction).
- *Scope's new surface — decided.* `Scope::resolve` is the one method gained.
  `Scope::dispatch` / `Scope::lazy_candidate` / `Scope::shape_pick` delete
  entirely rather than becoming wrappers — the scheduler is their only
  non-test caller and the whole point is removing the four-walk surface.
  [scheduler/finish.rs:38](../src/runtime/machine/execute/scheduler/finish.rs)
  updates to call `resolve` directly.
- *Scheduler driver phases — decided.* `run_dispatch` becomes a linear
  pipeline of four named phases: `try_short_circuit` (today's lines 60-83),
  `apply_auto_wrap` (lines 91-110, pure transform), `try_replay_park` (lines
  116-167), `schedule_deps` (merges today's lazy loop at 174-195 with the
  eager loop at 197-226, both driven by `Resolved`'s role indices). Final
  method signatures (`&mut self` vs free, `Option<NodeStep>` vs
  `Result`-flavored returns) are implementation details, not design choices —
  they settle against the existing `NodeStep` / `KError` flow.
- *`install_dispatch_placeholder` — decided.* Delete outright. The placeholder
  name lives on `Resolved` and the driver calls
  `scope.install_placeholder(name, NodeId(idx))` directly.
- *Test migration — decided.* `dispatcher.rs`'s ~300 lines of tests follow
  their subjects: tests exercising the chain walk / strict-then-tentative /
  ambiguity behavior move next to `Scope::resolve` in `scope.rs`'s test
  module; tests exercising `accepts_for_wrap` / `lazy_eager_indices` /
  `classify_for_pick` move into `kfunction.rs`'s test module against the new
  methods; the one test exercising `pick_most_specific_index` moves into
  `signature.rs`'s test module. Existing test names and assertions migrate
  verbatim where the new surface admits them; tests that only covered
  obsolete wrapper paths (e.g. `Scope::dispatch` forwarding to `dispatch`)
  delete with the wrappers.
- *Implementation ordering — decided.* Five steps, each leaving the tree
  green: (1) add `ExpressionSignature::most_specific` and migrate its callers
  inside `dispatcher.rs` to use it; (2) add the shape-predicate methods to
  `KFunction` and migrate the free-function call sites inside `dispatcher.rs`
  to use them; (3) introduce `Resolved` / `ResolveOutcome` and
  `Scope::resolve` alongside the existing entry points (no callers yet); (4)
  switch the scheduler driver and `finish.rs` callers to consume `resolve`,
  rewriting `run_dispatch` as the four-phase pipeline and deleting
  `install_dispatch_placeholder`; (5) delete `dispatcher.rs` and the
  now-orphaned `Scope::dispatch` / `lazy_candidate` / `shape_pick`
  forwarders. Step 1 and step 2 are pure moves and run first; the scheduler
  rewrite waits until `Resolved` exists so each step is a small green
  diff.

## Dependencies

**Requires:** none — runs against shipped infrastructure
(`DepGraph` / `NodeStore` / `WorkQueue` from `8f637d1` / `1ebcccd` / `273721f`,
the existing dispatch-as-node scheduler, and the dispatch-time name
placeholder semantics in
[design/execution-model.md § Dispatch-time name placeholders](../design/execution-model.md#dispatch-time-name-placeholders)).

**Unblocks:** none strictly — but
[simplify-and-shrink-context.md](simplify-and-shrink-context.md) lists
"pulling `dispatcher` out of `core`" as one of the candidate partitions for
its structural reshuffle; this item is a stronger move (delete the module,
don't relocate it) that supersedes that line and removes
[dispatcher.rs](../src/runtime/machine/core/dispatcher.rs) from its 600+-line
file split list.
