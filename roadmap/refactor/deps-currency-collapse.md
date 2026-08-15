# Collapse the Deps owned/park currency

**Problem.** Koan's `Deps` / `DepResults` currency carries the two-list
`[park..., owned...]` layout with split positional addressing — `park(i)` /
`owned(j)` accessors, `Slot::Park` indexing in
[literal.rs](../../src/machine/execute/dispatch/literal.rs), and the
`own(...)` / `park_on(...)` builder split
([scheduler-library.md](../../design/scheduler-library.md)) — but under
edge-centric delivery the scheduler consumes no park/owned distinction: deps
arrive as ordinary residents regardless of role. Two index spaces persist for
one dep list, and the two sides now differ in currency as well: a `Deps` park
is a source `ProducerId` the embedder holds, an owned entry a request the harness
realizes to a producer, and `ResolvedDeps` is a separate producer-keyed struct
the install door writes. The two index spaces are self-sustaining: `park_on`
dedups by `EdgeId` to keep a builder-issued index stable, and the index is
needed because dedup makes the cell-to-dep mapping non-positional. The split
propagates outward into two wiring entry points, `install_deps` and
`alloc_node` / `alloc_node_with_parks`.

**Acceptance criteria.**

- `Deps` exposes one dep list and `ResolvedDeps` one edge list; `DepResults`
  no longer exists, and a finish reads its deps as a plain slice of terminals.
- No koan code branches on a dep's park-vs-owned role, and neither name appears
  in the dep currency, its builder verbs, or the docs describing it.
- Dep order is read order: the builder issues no index for a dep named by a
  held producer, and an aggregate literal's cells read their deps through a
  cursor rather than a stored slot index.
- Every dep is named by a source edge and inherits that source's destination —
  one rule, applied by one install door reached through one allocator entry
  point.

**Directions.**

- Dep-entry currency — decided. One list of `Dep::Producer(EdgeId) |
  Dep::Request(R)`. The surviving axis is realization phase, not role: whether
  the caller can name the dep yet or the harness must spawn it. It dies at the
  apply harness, past which every dep is one `EdgeId`.
- Residual roles — decided. None. Short-circuit, catch, and resume routing read
  no per-dep role, so the positions alone carry it.
- `DepResults` — decided. Deleted. With the park prefix gone it is a newtype
  over `&[T]` carrying no invariant.
- Held-producer dedup — decided. Dropped, since it existed only to keep a
  builder-issued index stable. A repeated held name parks twice; both edges
  inherit one destination, so the delivery walk's per-destination dedup
  collapses them to one adopt.
- Spawned-dep wiring — decided. The harness mints a transient source edge
  destined at the consumer's anchor region and releases it once the door has
  minted the consumer's own edge off it. Costs one recycled slab entry per
  spawned dep and keeps the door the sole minter of a consumer's dep edges,
  rather than moving mint policy koan-side.

## Dependencies

**Requires:**


**Unblocks:** none — leaf.
