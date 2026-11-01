# Placing a verdict without probing

A recorded relation verdict routed to its tier by what the relation already
read.

**Problem.** `record_verdict`
([`chain.rs`](../../lattice/src/types/chain.rs)) places every verdict it
records through `tier_of` on both handles. For a handle outside the root,
`tier_of` probes the agent tier — a dynamic call, a `RefCell` borrow and a
hash probe — and then the region tier, before settling on the own tier. Every
relation that misses the verdict cache and records its answer — the order in
[`order.rs`](../../lattice/src/types/order.rs), member admission in
[`unify.rs`](../../lattice/src/types/unify.rs) and class ranking in
[`ranking.rs`](../../lattice/src/types/ranking.rs) — pays those probes twice
over, after `verdict` has already probed the own, region and agent tiers for
the pair. A handle has no free bit to carry its tier, and a `Linked` node
reference does not say which tier holds it.

**Acceptance criteria.**

- Recording a verdict probes no tier table to place it.
- No verdict is recorded in a tier that outlives either of its two types,
  and the lattice's verdict tests pass.
- A test in the lattice's
  [verdict tests](../../lattice/src/types/tests/verdicts.rs) counts the tier
  probes recording a verdict makes over a pair of region and own types, and
  counts none.

**Directions.**

- *Where the tier comes from — open.* The relation's caller passes the
  `Placed` of each side, learned where it resolved the handle to its node;
  or a node records its tier, at a byte per stored node; or every verdict
  over a pair not both in the root is recorded in the own tier, the
  shortest-lived the chain writes, trading reuse across a region's frames for
  no probe. Each changes the lattice's surface, which needs the user's
  approval before it lands. Recommended: the caller's `Placed`, if every
  relation's caller resolves both nodes before relating them.

## Dependencies

**Requires:** none.

**Unblocks:** none.
