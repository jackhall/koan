# Unified walk + strict-only admission

Reduce dispatch to a single ancestor walk that co-resolves function
candidates and bare-name arguments, replace strict-then-tentative
admission with strict-only, and add a no-keyword fast lane that bypasses
the candidate machinery entirely for the three shapes that have no
candidates.

**Problem.** A keyword-headed call today does one ancestor walk in
[`resolve_dispatch`](../../src/machine/core/resolve_dispatch.rs) to pick
a function plus one ancestor walk per bare-name wrap/ref-name slot in
[`resolve_name_part`](../../src/machine/execute/scheduler/dispatch.rs)
Phase 3. A call with N bare-name slots costs 1 + N walks. The two-pass
admission (strict peeks bare names; tentative admits them blind)
compounds the unpredictability: which overload wins depends on whether
each arg is a name, literal, or forward reference; inner-scope tentative
shadows outer-scope strict; the `Deferred` and `ParkOnProducers`
outcomes are buried inside the tentative pass. No-keyword expressions —
`(xs)`, `(Number)`, `(List Number)`, `(f 7)` — go through the same
candidate machinery though they have no candidates; only
single-`Identifier` is short-circuited today in
[`try_short_circuit`](../../src/machine/execute/scheduler/dispatch.rs).

**Impact.**

- *One ancestor walk per call site.* The unified walk visits each scope
  on the caller's `outer` chain once, performing function-bucket lookup
  and bare-name slot resolution together. 1 + N reduces to 1; park-wake
  replay stays separate.
- *Strict-only admission.* The strict pass becomes the only admission
  rule. Strict-Empty at every scope branches explicitly on the bare-name
  args' resolution state: eager parts → `Deferred`; `Placeholder` →
  `ParkOnProducers`; `Unbound` → `UnboundName`; otherwise `Unmatched`.
  Binders admit under strict unchanged — their critical slots are
  `Identifier` and `KExpression`, neither of which peek.
- *No-keyword fast lane for three flavors.* Token-shape classification
  before any walk: single bare token (Identifier or Type) → direct
  `Scope::resolve` / `resolve_type`; ≥2 Type-tokens (`(List Number)`)
  → small type-call evaluator; lowercase-Identifier head + non-keyword
  body (`(f 7)`) → resolve head to `KFunction`, bind args directly.
  Keyword = all-caps Identifier; qualified paths expand to ATTR at
  parse time and stay on the candidate path.
- *Specificity is a per-scope tiebreak.* Innermost-scope wins; ties at a
  scope break by slot-specificity. Cross-scope ranking collapses to
  lexical-scoping intuition: the nearest enclosing definition wins.

**Directions.**

- *Token-shape classification on the dispatch node — open.* Parse-time
  decidable; compute once on `Dispatch` construction, branch on it in
  `run_dispatch`.
- *Unified-walk slot-resolution contract — open.* At each scope level
  the candidate-bucket lookup and the bare-name slot resolution share
  the same scope handle; the candidate-pick commits only when every
  bare-name slot has a `Value` outcome or a `Placeholder` to park on.
- *Strict pass reads slot-resolution outcomes — open.* Strict admission
  inspects each bare-name slot's outcome (from the unified walk) rather
  than re-peeking. `Value` admits on carried type; `Placeholder` feeds
  `ParkOnProducers`; `Unbound` surfaces `UnboundName` immediately.
- *Drop tentative — decided.* `signature_admits_tentative` and the
  `PickPass::Empty → pick_tentative` arm in `resolve_dispatch` go away;
  the strict-Empty branches above replace them.
- *Collapse the five `Bindings` raw-map accessors into three
  visibility-aware lookups — shipped.*
  [`Bindings::lookup_value`](../../src/machine/core/bindings.rs),
  `lookup_type`, and `lookup_function` each take a
  `chain_cutoff: Option<usize>` and apply the `visible` predicate inside
  the lookup. `lookup_function` returns
  `FunctionLookup::{Bucket(Vec<&KFunction>), Pending(NodeId), None}`,
  pre-filtered for per-overload visibility, and covers the bucket /
  `pending_overloads` fall-through in one per-scope call so the
  dispatcher's ancestor walk is single-pass — consumers no longer see
  `BindingIndex` at the call site. `Scope::resolve_with_chain` /
  `resolve_type_with_chain` and `resolve_dispatch`'s ancestor walk
  delegate to the per-scope lookups; the five raw map accessors
  (`data` / `types` / `functions` / `placeholders` / `pending_overloads`)
  are gated `#[cfg(test)]`, and production sweeps go through the
  value-yielding `iter_data` / `iter_types` / `iter_functions`.

## Dependencies

**Requires:** none. Nested-binder recursive submission shipped, so a
sibling dispatching before its sibling's binder slot pops still finds
the placeholder and parks rather than hard-erroring under strict-only
admission.

**Unblocks:** none — leaf simplification.

The shipped behavior collapses to one-paragraph dispatch semantics:
innermost scope wins; ties within a scope by slot-specificity;
bare-name args' resolution state drives the strict-Empty fallback.
