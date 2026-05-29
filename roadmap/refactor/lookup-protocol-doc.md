# Canonical doc for the lookup → admit protocol

Write `design/typing/lookup-protocol.md` as the single named owner of
the three-layer protocol that every dispatch and name-resolution site
threads. The code layers are correctly distributed (scope routes,
bindings stores, predicates admit), but the contract is restated in
adjacent prose across four design docs.

**Problem.** Every dispatch and name-resolution site threads the same
three layers: `Scope` finds the ancestor by chain-walk, `Bindings`
finds the entry in the map, `KType` predicates accept or reject the
candidate. The participants are:

- [`Scope`](../../src/machine/core/scope.rs) — `resolve_with_chain`,
  `resolve_type_with_chain`, `resolve_dispatch_with_chain`.
- [`Bindings`](../../src/machine/core/bindings.rs) — `lookup_value`,
  `lookup_type`, `lookup_function`, the `visible` predicate consumer.
- [`KType` predicates](../../src/machine/model/types/ktype_predicates.rs)
  — `accepts_part`, `is_more_specific_than`, `matches_value`.
- [`resolve_dispatch`](../../src/machine/core/resolve_dispatch.rs) —
  `signature_admits_strict`, `OverloadBucket::pick`.

The contract — "scope finds the ancestor, bindings finds the entry,
predicates accept or reject" — is restated in adjacent prose across
[`elaboration.md`](../../design/typing/elaboration.md),
[`ktype.md`](../../design/typing/ktype.md),
[`user-types.md`](../../design/typing/user-types.md), and
[`execution-model.md`](../../design/execution-model.md). The four-doc
co-citation is the strongest mechanical signal in the
`doclinks gap` analysis. The doc co-citation is real; it just doesn't
correspond to a code-level seam — scope → bindings → predicates is a
correctly-layered foundation, and the candidates analysis confirmed
that wrapping it in a `core::lookup` module costs more in coupling
than it saves (Pass 15 Δ +5.46 even with doc consolidation). The
seam exists only at the doc level.

**Impact.**

- One canonical page names the three layers and lists the entry
  points: `resolve_*_with_chain`, `lookup_*`, `accepts_part` /
  `is_more_specific_than` / `matches_value`,
  `signature_admits_strict`.
- The four current docs each drop their lookup-protocol restatements
  to a single cross-link. Their topic-specific prose stays in place;
  only the per-doc protocol paraphrase shrinks.
- The `doclinks gap` signal for the scope / bindings /
  ktype_predicates / resolve_dispatch triple drops, retiring the
  strongest mechanical "look here for a seam" signal in the
  codebase.
- A reader investigating name resolution learns the layer protocol
  once and then reads each docs's topic-specific consequences,
  rather than reconstructing the contract from four restatements.

**Directions.**

- **Doc-only seam — decided per Pass 15.** The code seam was
  rejected (Δ +5.46) because the layered foundation is correctly
  distributed; the doc seam stands alone.
- **Foundation framing — decided.** The page names this as a
  *foundation* (correctly cited everywhere because every operation
  goes through it), not a *seam* (concept restated across docs
  because no source file owns it). The framing is the headline; the
  layer enumeration is the substance.
- **Inbound link rewrites — decided.** Each of the four current
  docs keeps its topic-specific content; their lookup-protocol
  restatements trim to a `see [design/typing/lookup-protocol.md]`
  reference.

## Dependencies

**Requires:** none.

**Unblocks:** none.
