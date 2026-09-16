# Slab cells without parents

The parent relation belongs to the tree pool. Delete it from the slab.

**Problem.** `create` takes an optional slab parent, and a slab cell created
under one gets a **birth row**: a square bit matrix beside the pin relation
([../src/matrix.rs](../src/matrix.rs)), transitively closed at creation so the
row names every ancestor. That relation gates three things — `disposable` reads
its column tally, `dispose_chain` walks its parent links, and `redeem` reads its
row to entitle a descendant to an ancestor's dormant carrier
([../src/graph.rs](../src/graph.rs)).

Nothing outside the crate creates the shape. Every `create` call in koan's
`src/` and in `workgraph/` passes `None`; the only callers that pass a slab
parent are cellgraph's own tests and the `birth_chain` shape of its
[perf harness](../perf/shapes.rs), which measures the cascade itself. A
call subtree's parent chain lives in the [tree pool](../src/tree/README.md),
whose `Ancestry` is the parent relation embedders actually use, and whose cells
are in no matrix at all.

So the slab carries a whole relation — a `512·W² + 256·W` byte matrix, a holder
tally, a per-slot parent link, a disposal cascade and a `redeem` clause — for a
shape nothing constructs. Its cost is the smaller half of the problem: while the
relation is there to be read, the slab reads as a habitat with an ancestry
structure, and design work keeps being written against a chain that does not
exist.

**Acceptance criteria.**

- `create` takes a continuation and nothing else. A slab cell has no parent, and
  `SlabCell::parent` is gone.
- `Matrix` is instantiated once, for the pin relation.
- A slab slot is disposable exactly when no tree child under it is undisposed,
  and `disposable` reads that count alone.
- A released slab cell disposes within its own `release` unless a tree child
  under it is undisposed. There is no slab cascade: the only walk left is the
  pool's, which ends at its root.
- Redeem entitlement for a slab home is home identity, the executing cell's pin
  row, or a sealed hold it carries. A cell with no such claim is refused
  `Unheld`.
- The tree pool's redeem clause for a slab home reads the root's pin row and
  sealed holds only ([../src/tree/README.md](../src/tree/README.md#redeem)).
- `dispose_chain`, `Matrix::inherit_row`, `Matrix::row_contains`,
  `Matrix::set`, `Bits::contains_all`, `CreateError::StaleParent` and every
  item the deletion leaves with no caller are gone. `Matrix::set` is deleted
  rather than kept under `cfg(test)`: the tally test scripts the writes the
  crate makes — `hold`, `clear`, `clear_row` — and no others.
- The tests that exercised slab parenthood are deleted rather than relocated:
  the birth entitlement and birth-row wait cases, the parented creations in
  [tests/surface.rs](../tests/surface.rs), and the property generator's parent
  draw. A test that only borrowed parenthood to hold a cell dead is repointed
  at an undisposed tree child instead, which is the state that survives.
- The `birth_chain` shape and its entry leave [the perf harness](../perf/shapes.rs)
  with no replacement: the only cascade left is the pool's, which `tree_chain`
  already measures end to end. The readings already in
  [observe/perf.csv](../observe/perf.csv) stay and roll off with the record's cap.
- [../README.md](../README.md) and
  [graph/README.md](../src/graph/README.md) state one relation — pins — and site
  the parent relation in the pool. No doc claims the slab has a birth column, a
  birth row, or a second matrix.

**Directions.**

- *The parameter is dropped, not rejected — decided.* `create` loses the
  argument rather than keeping it and refusing `Some`. A parameter that only
  ever takes `None` is exactly what invites designing against a relation that
  is not there.
- *Birth entitlement is deleted, not relocated — decided.* A descendant
  redeeming an ancestor's dormant carrier can only arise in the pool, where
  root identity already entitles it in O(1).
- *Dead-but-undisposed survives — decided.* A released root with undisposed
  tree children still waits in the slab with its storage intact. That state
  belongs to the pool's child count, which outlives this deletion.

## Dependencies

The ancestor-brand work this clears the ground for is a tree-pool question once
the slab has no chain — see [tree/README.md](../src/tree/README.md#the-ancestry-rule),
which already carries the rule a brand would ride on.

**Requires:** none — the relation and all three of its readers stand today.

**Unblocks:** none.
