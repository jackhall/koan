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

- A per-type match cache records, keyed by a candidate supertype's `Unique` digest, whether the
  type satisfies it; a repeat check hits in O(1) rather than re-walking.
- Both satisfied and unsatisfied outcomes are cached, so neither a repeat match nor a repeat
  non-match re-walks.
- Cached outcomes are never invalidated (types are immutable, so the relation is fixed).
- A check whose self or candidate digest is `Collided` bypasses the cache and walks
  structurally.
- The cache lives in the type's registry entry (one per distinct type), shared across all
  `KType` instances of that type, and merges up on lift with the rest of the entry.

**Directions.**

- *Cache in the registry entry, keyed by digest — decided.* Not on the `KType` value: `ktype()`
  rebuilds a fresh `KType` constantly, so a per-value cache never warms; the digest-keyed
  registry entry survives reconstruction and shares hits across instances.
- *Memoize positive and negative outcomes — decided.* A failed match is as worth caching as a
  successful one, given overload resolution's many non-matching slot checks.
- *`Collided` bypass — decided.* A `Collided` digest is not a reliable key; fall back to the
  structural walk, as equality does.
- *Result caching only, no transitive inference — deferred.* Deriving `T <: U` from `T <: S`
  and `S <: U` (lattice closure) is a separate, heavier feature.
- *Cache growth bound — open.* An unbounded per-type vector versus a capped/LRU cache; the
  hot-type worst case decides.

## Dependencies

**Requires:** [Content-addressed type identity](type-identity-registry.md) — the digests key
the cache, and the registry entry is its home.

**Unblocks:** none tracked yet.
