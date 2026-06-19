## Design tree

Design rationale for Koan, partitioned by concern. Each doc owns one
topic end-to-end; this index says which doc owns what so future edits
land in the right place by partition rather than by intuition.

The root-level docs cover the six cross-cutting concerns of the
runtime (execution, memory, parsing, error handling, functional
programming, monadic effects). The [typing/](typing/README.md)
subdirectory carries the type-and-module system as one topic because
both share the scheduler-driven elaborator and the nominal-identity
carrier — see [typing/](typing/README.md) for its own index.

## Doc index

Root concerns:

- [execution/](execution/README.md) — the three-stage
  parse → dispatch → execute pipeline, the deferred-dispatch
  scheduler with `pending_deps` / notify-list wakeups, tail-call
  rewriting, and the per-call `KoanRegion` lifecycle.
- [memory-model.md](memory-model.md) — value ownership through
  `KoanRegion` / `CallFrame`, the storage shape, lexical scoping,
  region lifetime erasure, and the re-entrant-scope-write protocol.
- [per-call-region/](per-call-region/README.md) — the
  single owner of the `Rc<CallFrame>` contract: anchor carriers,
  lift-time anchor decision, the `alloc_object` cycle gate, active-frame
  propagation, the `outer_frame` chain for builtin-built frames, TCO
  frame reuse, and the ping-pong reserve rotation.
- [expressions-and-parsing.md](expressions-and-parsing.md) — the
  parse pipeline (quotes → whitespace → expression tree → tokens →
  operators), the `KExpression` shape it produces, the
  eager-by-default evaluation rule, and how `FN` definitions extend
  the surface syntax without macros.
- [functional-programming.md](functional-programming.md) — functions
  as first-class `KObject` values, signature-driven evaluation,
  tail-call optimization, and the FN-as-extension-mechanism property.
- [error-handling.md](error-handling.md) — `KError` as a value
  propagating through the scheduler's dependency edges, frame
  attribution, the `TRY … WITH` arm shape, the per-arm `it` binding,
  and the privilege boundary that keeps builtin and user errors
  disjoint.
- [effects.md](effects.md) — the in-language monadic side-effects
  design: a `Monad` signature in Koan, concrete effect modules
  (`Random`, `IO`, `Time`) ascribing it, and the dispatch story for
  bind/return. Tracked in
  [roadmap/libraries/monadic-side-effects.md](../roadmap/libraries/monadic-side-effects.md).

Type and module system ([typing/](typing/README.md)):

- [typing/](typing/README.md) — the typing subdirectory's
  own index and the properties of the design (multi-abstract-type
  implicit resolution, higher-kinded abstraction, scoped coherence,
  versioning by import).
- [typing/tokens.md](typing/tokens.md) — the parser-level
  Keyword / Type / Identifier split that lets type names occupy a
  syntactic slot without quoting.
- [typing/ktype/](typing/ktype/README.md) — `KType` variants, container
  parameterization, variance, type-position slot kinds, function
  signatures, and dispatch / slot-specificity.
- [typing/elaboration.md](typing/elaboration.md) — how a type name
  resolves to a `KType` through the scheduler-driven elaborator: strict
  source-order resolution (a forward type reference is a position error),
  the binding-map partition, the `KType::Unresolved` bare-leaf transient,
  the resolution memo, and the `RECURSIVE TYPES` block for mutual
  recursion.
- [typing/user-types.md](typing/user-types.md) — the `RecursiveSet`
  nominal model: a `KType::SetRef` member is the per-declaration identity
  for named UNION, MODULE, opaque ascription, and NEWTYPE; the
  schema filled in the set member; the `OfKind(KKind)` family-kind slot; the
  type-only finalize install through `Scope::register_type_upsert`; the
  `RECURSIVE TYPES` block for mutually recursive nominals.
- [typing/lookup-protocol.md](typing/lookup-protocol.md) — the
  three-layer foundation (`Scope` chain-walk → `Bindings` per-scope
  lookup → `KType` predicate admit) every dispatch and name-resolution
  site threads, named here once so the topic docs can cross-link rather
  than restate.
- [typing/modules.md](typing/modules.md) — `MODULE` / `SIG`, the
  transparent / opaque ascription operators (`:!` / `:|`), and
  first-class module values flowing through `LET`, ATTR, and function
  calls.
- [typing/functors.md](typing/functors.md) — modules parameterized by
  modules: surface vs machine semantics, per-call generativity,
  deferred return types, higher-kinded type-constructor slots, and
  the `WITH` infix builtin family.
- [typing/implicits.md](typing/implicits.md) — implicit module
  parameters, lexical resolution, axioms with property-tested
  checking, cross-implicit equivalence checking, and the
  resolution-and-coherence design dials.
- [typing/scheduler.md](typing/scheduler.md) — type inference and
  implicit search as ordinary `Dispatch` / dep-finish scheduler work
  rather than a parallel node-kind track.
- [typing/type-language-via-dispatch.md](typing/type-language-via-dispatch.md)
  — the `:(...)` sigil as a parse-context marker; parameterized type
  construction (`LIST`, `MAP`, `FN`, `Functor`) and user-defined
  functor application as keyworded overloads sharing the value-side
  candidate-bucket and binder-admission machinery.
- [typing/open-work.md](typing/open-work.md) — roadmap pointers for
  the module-system stages plus the cross-cutting standard-library,
  group-operators, and JIT items.

## Foundation vs seam

Two patterns produce different doc-citation shapes and need different
responses. The [doclinks signals](../tools/doclinks.py) audit and
[modgraph](../tools/modgraph/) score together can tell them apart
mechanically; the heuristic below names what each is testing for, so
future refactor analysis starts from the right question.

A **foundation** is a source file every operation in some concern
*has* to go through — name resolution threads
[scope.rs](../src/machine/core/scope.rs) →
[bindings.rs](../src/machine/core/bindings.rs); allocation goes
through [arena.rs](../src/machine/core/arena.rs); nothing storable
exists without an [`arena.rs`](../src/machine/core/arena.rs) entry. A
foundation is *correctly* cited everywhere — every doc touching the
concern has to name it, because the concept the doc is explaining
genuinely passes through that file. The doc co-citation signal
(`scope.rs` mentioned in 11 docs; top doc holds 24% of mentions) looks
like sprawl but is the foundation's centrality leaking into the
audit. Wrapping a foundation in a sub-module *adds* coupling cost
(every caller now threads an extra layer) without dissolving the
underlying through-traffic. The 2026-05 candidate analysis tested
this on `core/lookup/` (rejected, +0.4) and `core/scope/` (rejected,
+0.2 residual) — both were genuine foundations the metric correctly
refused to wrap.

A **seam** is a *contract* restated across docs because no source file
owns it. The per-call region protocol (which carriers anchor a
`Rc<CallFrame>`, how lift attaches an anchor, where the alloc cycle gate
fires) spans five design docs and ~10 source files: the docs restate the
rule because no single source file holds the contract — the participants
implement the protocol independently. Two fixes are available: a
code-level seam (concentrate the participants in one module, scored by
`modgraph`) or a doc-level seam (a single canonical page the participating
docs cross-link to, with the code staying distributed). The per-call region
protocol took the latter shape — it stays code-distributed and gets a
single owner doc.

A third, stronger resolution is to *dissolve* the seam: fold the
duplicated state into a single carrier so the protocol stops being a
contract-by-convention at all. The nominal types take this path — a
`NEWTYPE` / `UNION` / `MODULE` / `Result` / `SIG` declaration carries its
schema (or signature) in its `KType` identity and writes only
`bindings.types`, so a reader finds the rule in one place (the identity owns
the schema) rather than as a dual-write contract restated across the typing
docs. SIG folds its constraint and value forms into one
`KType::Signature { sig, pinned_slots }` variant the same way, so no nominal
binder dual-writes — the seam is dissolved rather than merely documented.

A **straddle** is a strongly-connected component split across a module
boundary. Unlike a seam (a contract with no owner) or a foundation (genuine
through-traffic), a straddle is a *cycle* the cut bisects:
[`model::values::KObject`](../src/machine/model/values.rs) holds `core`
closure/scope types while [`core::scope::Scope`](../src/machine/core/scope.rs)
imports `KObject` back. The scorer charges its cross-boundary edges as
`α·feedback` — the weight of the edges you would cut to make the module graph a
DAG. You cannot layer a cycle, so a single-item move leaves the component
straddling and a module rename realigns nothing; only co-locating the whole
component into one module turns its cycle edges into free intra-module edges.
Expose one item from that module and `λ_facade` stays minimal too. If a member
genuinely cannot move, the fallback is to thin to a single facade item in
place — the cross edges then pay `λ` once, though the cycle cost remains.

The operational test: if pulling the items into a new module reduces
the metric (especially under paired doc consolidation), it's a seam.
If the metric goes up even after consolidation, it's a foundation —
the docs are right to cite it, and what the analysis surfaced as
"sprawl" is actually correct distribution. High doc co-citation is a
candidate-hunter, not a verdict.

## Analysis tooling

Three CLIs in [tools/](../tools/) drive refactor analysis against the
live source and doc graphs. Use them in this order when scoring a
proposed structural change:

- [`tools/doclinks.py signals`](../tools/doclinks.py) — surfaces
  mechanical doc-abstraction signals as JSON: src-file co-citation
  triples, backref density, comment-density spikes, shared
  n-gram phrases across docs. Start here to find candidates — high
  co-citation and shared-phrase signals point at concepts the docs see
  but the code-graph may not. Pair with `doclinks gap` for the
  doc-vs-code-graph delta.
- [`tools/modgraph score`](../tools/modgraph/) — scores the module
  tree against a cargo-modules DOT export with a fractal complexity
  metric (coupling + nesting + comprehension-aware per-file size +
  owner credit for documented protocols). The DOT is built by
  `tools/modgraph regen`, which re-attributes `uses` edges to the
  written import surface (re-export correction) so facade-routed
  imports aren't charged as deep coupling. The bottom-line score is
  the total cost over a fixed denominator (`--denominator`, default
  1000) — a constant scale, not the tree's LOC, so deleting code
  always lowers the score; use `--baseline FILE` to record runs to a
  tracked trend log.
- [`tools/modgraph rewrite item`](../tools/modgraph/rewrite.py) —
  SCIP-driven item-level what-if. Given an item (function, method,
  type) and a target module, produces a rewritten DOT + mirrored
  `src/` tree that can be scored against the baseline. Supports
  `--delete ITEM`, `--delete-file PATH`, and `--prose-redirect` to
  model paired doc consolidation alongside the code move. The
  item-level granularity is essential: whole-module renames usually
  drown the seam by dragging unrelated peers along.
- [`tools/modgraph propose`](../tools/modgraph/propose.py) — SCIP-driven
  co-location candidate generation. Builds the item-level directed
  graph and surfaces two kinds of group: **cycle** candidates
  (cross-module strongly-connected components, the `α·feedback`
  carriers) and **density** candidates (dense clusters from a
  modularity pass over the undirected projection, the `cross`
  carriers). Each candidate is scored by the co-locate what-if (its
  members into one synthetic module, via the same `rewrite item`
  pipeline) and the triage list is ranked by Δscore, most-negative
  first — a Δ>0 candidate is a foundation co-location would regress.
  `propose` only ranks; the human picks the cut.

Canonical command pattern for scoring a candidate against the
current baseline (the `--move` / `--prose-redirect` targets below are
placeholders — substitute the item and destination module under test):

```sh
python3 tools/modgraph regen --root koan --baseline observe/complexity.txt
python3 tools/modgraph rewrite item \
  --scip /tmp/koan.scip --edges observe/modules.dot --src-root src \
  --output-edges /tmp/candidate.dot --output-src /tmp/candidate_src \
  --move 'koan::machine::core::scope::Scope::SOME_ITEM=koan::machine::core::TARGET'
# --prose-redirect (a score-time flag) models paired doc consolidation:
python3 tools/modgraph score --edges /tmp/candidate.dot --src-root /tmp/candidate_src --root koan \
  --prose-redirect 'src/machine/core/scope.rs=src/machine/core/TARGET.rs'
```

The decision-log methodology — what counts as a measured win, when to
re-baseline after tooling fixes, how to tell a structural metric
"silence" from a real verdict — is the rubric every refactor-analysis
pass applies, with each pass building on the prior one rather than
restarting from scratch.

## Open work

- `modgraph propose` (above) now surfaces SCC (cycle) and modularity
  (density) co-location candidates, scored and ranked by the existing
  what-if. Its first intended target is the machine
  [model/core straddle](../roadmap/refactor/machine-straddle-colocation.md),
  still open.
