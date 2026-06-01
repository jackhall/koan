# The lookup → admit protocol

Every dispatch site and every name-resolution site in koan threads the
same three layers:

1. **`Scope` finds the ancestor** by walking the scope chain against
   the consumer's `LexicalFrame`.
2. **`Bindings` finds the entry** in that ancestor's per-kind map and
   gates it against the consumer's `chain_cutoff` via the
   `visible(b.nominal_binder || b.idx < c)` predicate.
3. **`KType` predicates accept or reject** the candidate against the
   slot's declared shape.

This page is the single named owner of the protocol. The participants
are correctly distributed across four source files — chain walk,
per-scope map lookup, and type-shape admission are each at their right
level — so wrapping them in a `core::lookup` module *adds* coupling
without dissolving the through-traffic. The 2026-05 candidate
analysis confirmed this (Pass 15, Δ +5.46 even with paired doc
consolidation). The seam exists only at the doc level: every typing /
dispatch doc has to mention the protocol because every operation in
its concern threads it. See
[design/README.md § Foundation vs seam](../README.md#foundation-vs-seam)
for the test that distinguishes the two patterns.

## Layer 1 — `Scope` chain walk

[`Scope`](../../src/machine/core/scope.rs) walks ancestor scopes once
per lookup, threading the consumer's
[`LexicalFrame`](../../src/machine/core/lexical_frame.rs) chain so each
ancestor receives the right `chain_cutoff`. Three entry points,
matching the three lookup kinds:

- [`Scope::resolve_with_chain`](../../src/machine/core/scope.rs) —
  value-name lookup. Per-ancestor calls
  [`Bindings::lookup_value`](../../src/machine/core/bindings.rs) and
  returns the first visible hit.
- [`Scope::resolve_type_with_chain`](../../src/machine/core/scope.rs) —
  type-name lookup. Per-ancestor calls
  [`Bindings::lookup_type`](../../src/machine/core/bindings.rs).
- [`Scope::resolve_dispatch_with_chain`](../../src/machine/execute/dispatch/resolve_dispatch.rs)
  — function-bucket lookup. Per-ancestor calls
  [`Bindings::lookup_function`](../../src/machine/core/bindings.rs)
  and, on a non-empty bucket, hands it to
  [`OverloadBucket::pick_strict`](../../src/machine/execute/dispatch/resolve_dispatch.rs)
  for the per-overload admit pass.

[`Scope::resolve`](../../src/machine/core/scope.rs) is the chainless
shorthand — it reads as "see everything" and is reserved for test
fixtures and builtin-registration paths. Production dispatch always
threads a chain.

## Layer 2 — `Bindings` per-scope lookup

[`Bindings`](../../src/machine/core/bindings.rs) owns the per-scope
maps — `data` (values), `types` (type-name → `&KType`), `functions`
(registered overloads), `placeholders` (in-flight name-keyed binders),
`pending_overloads` (in-flight bucket-keyed binders). The three
visibility-aware accessors are mutually exclusive per name:
`bind_value` and `register_function` remove their own placeholder
before inserting, so a binding never appears in both `data` /
`functions` and `placeholders` at the same moment.

- [`Bindings::lookup_value`](../../src/machine/core/bindings.rs)
  consults `data` then `placeholders`. Returns
  `Resolution::Value(&KObject)` for a finalized visible binding,
  `Resolution::Placeholder(NodeId)` for a still-running visible
  producer (the caller parks on it), or `None` (the caller surfaces
  `Resolution::UnboundName` on chain exhaustion).
- [`Bindings::lookup_type`](../../src/machine/core/bindings.rs) is the
  type-side symmetry: consults `types` then `placeholders` and surfaces
  the same three-arm result.
- [`Bindings::lookup_function`](../../src/machine/core/bindings.rs)
  consults `functions[key]` first, filtered per-overload by visibility,
  and falls through to `pending_overloads[key]` only when no live
  bucket admits. Returns `FunctionLookup::Bucket(Vec<&KFunction>)`
  (non-empty, pre-filtered), `FunctionLookup::Pending(NodeId)` (an
  in-flight FN / FUNCTOR binder's producer to park on), or
  `FunctionLookup::None`.

The visibility predicate is one line —
[`visible(b: BindingIndex, chain_cutoff: Option<usize>)`](../../src/machine/core/bindings.rs)
— shared across all three. `b.idx < c` is the strict
value-style gate; the `nominal_binder` carve-out lets `STRUCT` /
named `UNION` / `SIG` / `MODULE` / `FUNCTOR` declared names cross
the cutoff so mutual-recursive nominal references work.

## Layer 3 — `KType` predicates

[`KType` predicates](../../src/machine/model/types/ktype_predicates.rs)
decide whether a candidate's carried type satisfies the slot's
declared shape. The three predicates partition the work by where the
check fires:

- [`KType::accepts_part`](../../src/machine/model/types/ktype_predicates.rs)
  — admission predicate. Tests an `ExpressionPart` (typically a
  `Future(obj)` for a resolved bare-name slot) against a declared
  slot type during dispatch admission. The strict-only admission
  rules table lives at
  [elaboration.md § Strict admission rules](elaboration.md#strict-admission-rules);
  the cache it consumes is built once per `run_dispatch` and shared
  between the strict admit pass and the post-pick splice walk.
- [`KType::is_more_specific_than`](../../src/machine/model/types/ktype_predicates.rs)
  — specificity ranking. Ranks two slot types when multiple overloads
  admit the same call, used by `ExpressionSignature::most_specific` to
  break ties. Concrete carrier types outrank `KType::Any`; user-type
  identities outrank their `AnyUserType` wildcards. The full ranking
  rules and variance behavior live at
  [ktype.md § Variance](ktype.md#variance) and
  [user-types.md § Specificity stratification](user-types.md#specificity-stratification).
- [`KType::matches_value`](../../src/machine/model/types/ktype_predicates.rs)
  — runtime content check. Walks a runtime value's contents against a
  declared type at an ascription boundary (FN return, FN argument,
  `LET`). This is the only predicate that walks contents; the other
  two read carrier-type metadata in O(1).

The dispatch-admission glue is
[`signature_admits_strict`](../../src/machine/execute/dispatch/resolve_dispatch.rs),
which walks slot/part pairs and consults the per-`run_dispatch`
`bare_outcomes` cache — the strict-admission rules table at
[elaboration.md § Strict admission rules](elaboration.md#strict-admission-rules)
spells out which `NameOutcome` arms admit via `accepts_part`, which
admit shape-only, and which strict-reject. [`OverloadBucket::pick_strict`](../../src/machine/execute/dispatch/resolve_dispatch.rs)
wraps the filter-then-`most_specific` dance over a single scope's
visibility-pre-filtered bucket.

## Why this is a foundation, not a seam

A *seam* is a contract restated across docs because no source file
owns it — the per-call arena protocol was a seam that got a single
canonical doc owner, while the nominal dual-write was a seam *dissolved*
outright by folding each binder's two entries into one `KType` identity. A
*foundation* is a source file every operation in some concern *has*
to go through; it's correctly cited everywhere because the concept
the doc is explaining genuinely passes through that file. Wrapping a
foundation in a sub-module *adds* coupling without dissolving the
underlying through-traffic.

The lookup → admit protocol is a foundation. The three layers are at
their right level — chain-walk is scope-shaped, per-scope entry
lookup is bindings-shaped, type-shape admission is predicate-shaped —
and a `core::lookup/` module that wrapped them would force every
caller through an extra layer without removing any current
through-traffic. Pass 15 of the 2026-05 candidate analysis scored
this rewrite at Δ +5.46 even after consolidating the four-doc
restatements into one canonical page; the doc consolidation stands
on its own.

What each topic doc adds beyond this protocol:

- [elaboration.md](elaboration.md) — type-name resolution's
  five-layer pipeline (surface-form cache → scope-bound memo →
  elaborator → bare-leaf coercion → surface-name carrier) and the
  per-scope binding-map partition that separates type-name lookups
  from value-name lookups.
- [ktype.md](ktype.md) — `KType` variants, variance under the three
  predicates, container parameterization, and the overload-bucket
  visibility filter as it interacts with slot-specificity.
- [user-types.md](user-types.md) — nominal-identity install through
  `Scope::register_type_upsert`, the specificity stratification for
  `UserType` vs `AnyUserType` vs `Any`, and the
  `placeholders`-driven cycle close for mutually recursive nominals.
- [execution-model.md § Dispatch-time name placeholders](../execution-model.md#dispatch-time-name-placeholders)
  — how forward references park through the `placeholders` /
  `pending_overloads` tables and resume on producer finalize, plus
  the submission-time binder install that prevents `UnboundName` /
  `DispatchFailed` for not-yet-popped sibling binders.
