# Memoized subtype matching

Cache dispatch admissibility outcomes per type, keyed by the candidate supertype's digest, so
a repeat subtype check is an O(1) lookup instead of a structural walk.

**Problem.** Dispatch admissibility — does a value's type satisfy a slot's type? — is a
recursive structural walk over `KType` (the predicates in
[`ktype_predicates.rs`](../../src/machine/model/types/ktype_predicates.rs): `accepts_part`,
`record_value_more_specific`, function compatibility, variant/union refinement). Overload
resolution reruns these constantly: the same `(value-type, slot-type)` pair is re-walked on
every call to an overloaded name, and a value is checked against many non-matching slots per
dispatch. Content-addressed identity makes *equality* O(1), but subtyping is a lattice relation
a hash cannot encode, so matching still walks every time.

**Acceptance criteria.**

- A per-type match cache records, keyed by a candidate supertype's digest, whether the
  type satisfies it; a repeat check hits in O(1) rather than re-walking.
- Both satisfied and unsatisfied outcomes are cached, so neither a repeat match nor a repeat
  non-match re-walks.
- Cached outcomes are never invalidated (types are immutable, so the relation is fixed).
- The cache lives in the type's registry entry (one per distinct type), shared across all
  `KType` instances of that type; entries are re-derivable, so a dropped entry (frame
  death) costs a re-walk, never correctness.
- Overload specificity orders two distinct SIG-declared signature types structurally: a
  `:A` slot is more specific than a `:B` slot iff `A`'s schema is a strict `sig_subtype` of
  `B`'s (pin agreement included), with the outcome cached like every other subtype check —
  two different SIGs are no longer incomparable in dispatch tie-breaking.

**Directions.**

- *Cache in the registry entry, keyed by digest — decided.* Not on the `KType` value: `ktype()`
  rebuilds a fresh `KType` constantly, so a per-value cache never warms; the digest-keyed
  registry entry survives reconstruction and shares hits across instances.
- *Memoize positive and negative outcomes — decided.* A failed match is as worth caching as a
  successful one, given overload resolution's many non-matching slot checks.
- *Per-module `satisfaction_memo` folds in — decided.* `Module::satisfaction_memo` (the
  `sig_id`-keyed structural satisfaction cache dispatch consults) migrates into the digest-keyed
  registry entry when this item ships, leaving `Module` cache-free.
- *Cross-SIG specificity lands here, not earlier — decided.* The shipped
  [self-sig and empty-signature specificity arms](../../design/typing/modules.md#first-class-modules)
  order a module against a `Declared` signature or the `Empty` top; ordering two distinct
  `Declared` signatures needs a per-pair `sig_subtype` walk, which is only affordable once this
  item's cache memoizes it.
- *Phasing — decided.* Foundation phase (carries the risk): the cache home in the registry
  entry, with the `Collided`-bypass contract. Mechanical phases, each leaving the
  verify-koan slate green: wiring the predicate call sites, growth-bound tuning.
- *Result caching only, no transitive inference — deferred.* Deriving `T <: U` from `T <: S`
  and `S <: U` (lattice closure) is a separate, heavier feature.
- *Cache growth bound — open.* An unbounded per-type vector versus a capped/LRU cache; the
  hot-type worst case decides.

## Dependencies

**Requires:**


**Unblocks:** none tracked yet.
