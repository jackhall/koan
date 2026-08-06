# Bump-backed binding tables

Makes a scope's binding tables `Drop`-free while keeping them mutable and
O(1). Concerns the tables of
[src/machine/core/bindings.rs](../../src/machine/core/bindings.rs); the bump
they ride is the region's
([design/value-substrates.md § Untyped arenas](../../design/value-substrates.md#untyped-arenas-the-drop-free-end-state)).

**Problem.** `Scope::drop` is O(entries): every `String` key across the
binding tables frees individually, every overload bucket `Vec` frees, every
`HashMap` frees its bucket array, and the droppy payloads inside entries — the
overload summary and operator declaration `String`s, the `DispatchToken`'s
`Vec` — free one by one, at every frame death. The region bump can absorb all
of it: `hashbrown` (std's `HashMap` internally) accepts an `allocator-api2`
allocator, and workgraph's bumpalo already enables that feature, so a table's
buckets can live in the bump — where deallocation is a no-op — with the same
O(1) hash lookup, and the entry payloads can ride bumped `&str` / bumped
slices like every value-channel string already does. Churn is bounded: slot
reuse is in-place, and only a resize abandons the old bucket array as dead
bump bytes, capped by geometric growth.

**Acceptance criteria.**

- The durable tables (`types`, `data`, `functions`, `operators`, the SIG slot
  collector) are hashbrown maps backed by the region bump through
  `allocator-api2`, with bumped `&str` keys and allocator-backed bucket
  vectors.
- Table entries carry no `Drop`: the overload summary, the operator
  declaration key, and the dispatch token are bump-hosted (`&str` / `Copy`
  element slices), so dropping a table frees nothing and runs no per-entry
  glue.
- `Scope`'s remaining `Drop` is confined to the `region_owner` back-link, the
  root-only `out` writer, and the `ScopeKind` payloads — no binding-table
  state; frame death walks O(scopes), not O(entries).
- Per-table choice is explicit: a table where bump-backing is awkward stays a
  std map, with the reason stated where it is declared.
- Lookup stays O(1) hash on the resolve path — no persistent-structure
  substitution.
- Resize abandonment is the only dead-byte source: pending resolution
  overwrites in place, so a table's peak occupancy is its final binding count.
- The Miri slate gains coverage for hashbrown-over-bump under tree borrows.

## Dependencies

**Requires:**

- [`Region::bump_capacity`](../../workgraph/src/witnessed/region.rs) — shipped:
  the pin figure is read off the allocator, so these tables' off-door
  allocations are priced without a counted door.

**Unblocks:**

- [Scopes move into the region bump](bump-hosted-scopes.md) — `Scope` cannot
  skip its destructor until the tables have none.
