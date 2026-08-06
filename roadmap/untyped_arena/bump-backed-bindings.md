# Bump-backed binding tables

Reduces a scope's teardown cost to near-nothing while keeping its tables
mutable and O(1). Concerns the tables of
[src/machine/core/bindings.rs](../../src/machine/core/bindings.rs); the bump
they would ride is the region's
([design/value-substrates.md § Untyped arenas](../../design/value-substrates.md#untyped-arenas-the-drop-free-end-state)).

**Problem.** `Scope::drop` is O(entries): every `String` key across the
binding tables frees individually, every overload bucket `Vec` frees, and
every `HashMap` frees its bucket array — at every frame death. The region bump
can absorb nearly all of it: `hashbrown` (std's `HashMap` internally) accepts
an `allocator-api2` allocator, and workgraph's bumpalo already enables that
feature, so a table's buckets can live in the bump — where deallocation is a
no-op — with the same O(1) hash lookup. Churn is bounded: slot reuse is
in-place, and only a resize abandons the old bucket array as dead bump bytes,
capped by geometric growth.

**Acceptance criteria.**

- The durable tables (`types`, `data`, `functions`, `operators`, the SIG slot
  collector) are hashbrown maps backed by the region bump through
  `allocator-api2`, with bumped `&str` keys and allocator-backed bucket
  vectors; a table's `Drop` frees nothing.
- `Scope::drop` reduces to the `Weak` back-link and the root-only `out`
  writer — frame death walks O(scopes), not O(entries).
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
- [Pending bindings as a value state](pending-binding-union.md) — resolution
  must overwrite in place, or every resolved placeholder abandons its bytes in
  the bump.
- [Frame-owned scopes retire the typed cells](frame-owned-scopes.md) — the
  retype site that will cover the tables' lifetime moves once, not twice.

**Unblocks:** none tracked yet.
