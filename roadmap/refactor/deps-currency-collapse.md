# Collapse the Deps owned/park currency

**Problem.** Koan's `Deps` / `DepResults` currency carries the two-list
`[park..., owned...]` layout with split positional addressing — `park(i)` /
`owned(j)` accessors, `Slot::Park` indexing in
[literal.rs](../../src/machine/execute/dispatch/literal.rs), and the
`resolved.own(...)` / `park_on(...)` builder split
([scheduler-library.md](../../design/scheduler-library.md)) — but under
edge-centric delivery the scheduler consumes no park/owned distinction: deps
arrive as ordinary residents regardless of role. Two index spaces persist for
one dep list.

**Acceptance criteria.**

- `Deps`, `ResolvedDeps`, and `DepResults` expose one dep list and one index
  space; the `park`/`owned` accessor split is gone.
- No koan dispatch code branches on a dep's park-vs-owned role.

**Directions.**

- Residual roles — open. Whether short-circuit and catch routing need any
  per-dep role after the collapse, or the positions alone carry it.

## Dependencies

**Requires:**

- [Delivery at finalize](../../workgraph/roadmap/delivery-at-finalize.md) —
  the scheduler must stop consuming the split first.

**Unblocks:** none — leaf.
