# The build tree

A second tree of cells beside the run tree, holding what the program's text
decides, under the one drain.

**Problem.** The load writes a body's shape and the type channel's cells into
program storage at `'graph`
([program storage](../../src/memory/README.md#two-tiers-and-why-the-boundary-falls-where-it-does)),
freezes the type root, and runs as one owner-side pass before the first step
([the top level](../../src/program/README.md#the-top-level)): nothing of it is
work on the drain, so nothing interleaves it with value computation, and a
refused program has no cell to keep. A type the run builds lives in a run
cell's region branch or in the scratch branch a park lends
([the scratch branch as a cache](../../design/type-tiers.md#the-scratch-branch-as-a-cache)),
so a fact settled under a binding has no home that outlives the frame and is
shared across callers: the warm tier is one park's, and a second caller under
the same binding starts cold.

**Acceptance criteria.**

- A `build` module stands at `src/build/`, with
  [its README](../../src/build/README.md) as its design, and the graph holds
  two slab roots: the run root and the build root.
- The build root's step runs the load to quiescence before the top-level run
  cell's first value step, writing the written program's shapes, its type
  facts, the caching records and the root type tier, which rest in the
  owner's storage as they do today; the build root's region holds the
  root-level specialization table.
- A specialization is one seam under one observed binding: a tree cell under
  the build root or under another specialization, keyed by the binding's
  digest in its parent's table, whose keys are never removed and whose
  entry for an evicted child is renewed in place, nested in binding order
  along the lexical chain, and rebuilt on a miss.
- A run cell born under a binding holds one counted borrow of its
  specialization, taken at birth and deepened at each seam it crosses; a
  tail successor of the same callee keeps its predecessor's; every other run
  cell keeps today's lend; a tenant reads through its host's; the run root
  pins nothing of the build tree, no build cell is pinned by a run cell, and
  no build cell holds a handle to a run cell.
- A parent's table holds bare handles; a stale handle is a miss; an evicted
  child waits dead but undisposed until its last borrower disposes.
- Type tiers chain longest-lived first as the root tier, the specialization
  a run cell's borrow names, region, scratch; a build cell lays type nodes
  over the root and itself alone; a per-call type reaches a region only by
  adopt at an exit; a verdict lives in the shortest-lived tier holding one
  of its types.
- A cold birth costs one build step and no park: the drain creates the
  specialization ahead of the frame, the specialization's step writes itself
  into its parent's table, and the frame is born borrowing it.
- A sealed body's specialization may hold minted content and a porous body's
  holds only what its exits adopt out; the porosity tests pass unchanged.

**Directions.**

- *Which bindings make a specialization* — decided. Variance is the first
  input: closed content names none, invariant content and a `FOR ALL` group
  solved to closed points name a shared one, varying content resumes into the
  run cell's scratch. Porosity, exposure and observed demand may enter the
  decision; it is not variance alone.
- *The run-to-build reference* — decided. A counted borrow, cellgraph's
  named lend ([tree cells](../../cellgraph/src/tree/README.md#death)), whose
  creation names its lender; never a matrix pin. A tenant reads through its
  host's.
- *Where the lookup runs* — decided. In the drain, when it gives a frame its
  cell: it reads the parent's table through a read-only peek and, on a miss,
  creates the specialization and runs its one step before the frame, so no
  step probes and no frame parks for it. A parent itself evicted births the
  frame with no specialization; rebuilding the chain above is the eviction
  policy's question.
- *Where shapes and the root tier rest* — decided. In the owner's storage,
  written by the build root's step. Moving them into the graph would need a
  region brand that outlives every other cell's, judged overkill for a
  cellgraph lifetime, and a digest-table root tier, whose slower miss is
  unwanted.
- *Where a parked fact lives* — decided. Data in the build cell's region,
  never a cell of the graph.
- *Eviction policy* — open. The parent prices warmth against the child's
  bytes; cellgraph ships the bytes and no threshold, as the spare list's
  bound against recent demand does. What a parent counts as warmth, and
  whether a borrower count enters the price, is unsized.
- *Sibling redundancy* — open. An inner body's facts whose frontier names
  only the inner rigid are re-solved once per outer binding. A hoist to a
  child of the root would share them at the cost of a second parent; left as
  cache the eviction policy prices until a program shows the cost.

## Dependencies

A refused written program's step stops at the refusal; keeping what it
wrote is what
[a refused program stays loaded](../rewrite/refused-programs-stay-loaded.md)
asks for. It builds on cellgraph's [tree cells](../../cellgraph/src/tree/README.md),
their named lend and peek, and [the type tiers](../../design/type-tiers.md).

**Requires:** none — cellgraph's substrate stands.

**Unblocks:**

- [Monotone elaboration](../gradual-typing/monotone-elaboration.md) — the
  solver's products need the build cells to land in.
- [Activation typing](../gradual-typing/activation-typing.md) — a frame's
  specialization is a build cell.
- [Code splicing](../metaprogramming/code-splicing.md) — composed code's
  shape lives in a composed-code cell under the build root.
