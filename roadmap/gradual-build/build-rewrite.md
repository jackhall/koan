# The build as the second embedder

A gradual build written in `src/build` that decides everything the run
executes, on cellgraph cells with the ordinary step protocol.

**Problem.** The build tree is a cellgraph tree cell driven by two side paths
of the drain: the load door enters the build root through a raw `enter` with
no `Step` ([`load.rs`](../../src/scheduler/load.rs)), and a missed
specialization is entered off the ready stack, synchronously, through
`enter_beside`, ending `Quiesced` as no other cell may
([`drain.rs`](../../src/scheduler/drain.rs), `seek`). Its typing context —
which specialization's facts a frame reads — rides cellgraph's
lend of a writable half, fixed at the frame's creation
([tree cells](../../cellgraph/src/tree/README.md#death)), so a `Shares`
frame's lend is counted on its host's scratch until that scratch resets, a
lend can close a wait cycle no creation refuses, and no frame reads a fact
settled mid-body. The compiler is three passes — `scope`'s shapes,
`elaborate`'s type channel and `dispatch`'s static pass — made re-entrant by
a walk that records each fact's edges as it reads
([the solver](../../src/build/README.md#type-facts-and-the-solver)), and the
evaluator re-derives the build's typing per read: `bound_path`,
retype-and-judge, and ranking at a use no key covers. The region header is
one record of four modules' concerns — branch, table, home, settled — which
the drain mutates between steps through `peek`, a door cellgraph documents
as read-only. Cellgraph grew `enter_beside`, `peek`, `is_childless`, named
and copied lends, counted tenant lends and `lend_cycle` for this one tree,
each with one caller or none.

**Acceptance criteria.**

- The evaluator re-derives no typing the build decided: at every site it
  reads the build's decision — the candidate, the route a value's memo
  takes, what a barrier checks — and no per-read `bound_path`,
  retype-and-judge or ranking path remains.
- A shape is built once per code value and a fact once per (frontier class,
  binding): a binding rebuilds no shape, and new code re-solves no fact of
  other code.
- A build cell is one frontier class under one binding, keyed on every name
  its seam binds, and the dispatch suite's confluence law holds across
  bindings taken at different seams.
- A fact no entry key covers is settled in a build cell born at its
  statement seam, keyed on the binding's carried type and borrowed by the
  frame for the remainder of its body. A loop whose statement binds a stable
  type makes one such cell; one whose statement mints per hop holds O(1)
  build cells across the loop.
- Live build memory is bounded by one budget over the whole build tree,
  evicted least recently used, leaf first, and the live set is independent
  of seam nesting depth.
- A build cell is born by `spawn`, may park, and ends by resting its facts
  in its own region as a storage-only cell; a build step that evaluates
  deterministic code parks as any step does; the load is the first root work
  over the build root.
- A run cell's borrow of a build cell is read-only and drops at the
  borrower's release, tenants included. A branch's nodes reference only the
  tiers of its own chain, and content read through a borrow is relaid
  locally before anything is built over it.
- A specialization table's entries are laid by a tenant step of the parent;
  a hit's stamp is written through the borrow under the header's own
  contract; the drain reads no header between steps.
- Cellgraph's public surface holds no load door, `enter_beside`, `peek`,
  `is_childless`, `Lend`, counted tenant lend or `lend_cycle`, and
  [`tests/surface.rs`](../../cellgraph/tests/surface.rs) names what remains.
- `src/build` owns shape building, the fact graph, selection, the site
  classes and the keys, with two surfaces: what the evaluator reads per site
  and what the drain needs to birth a frame. The property suites of the
  passes it replaces hold against it.

**Directions.**

- *Build decides, run executes — decided.* The build/run equivalence — the
  run re-deriving the build's typing per read — goes. *Resume equals
  restart* ([properties](../../src/build/README.md#properties)) stays, as a
  law of the build.
- *Two caches, two keys — decided.* A shape is a function of text, built once
  per code value: the written program's first and program-long, composed
  code's at `EVAL` and counted by its home
  ([code splicing](../metaprogramming/code-splicing.md)). A fact is a
  function of shape and binding. Neither re-runs for the other's reason, so
  the composed-code cell is a shape cell, not a kind of its own.
- *The grain of a fact — decided.* Coarse: a unit's facts run as one
  function. A fact's edges are to unknowns and to earlier facts, and the
  frontier is what the graph is exact about; a segment is a shape's name,
  not a grain of solving.
- *Seam-per-statement — decided.* A fact no entry key covers resumes in a
  build cell born at the statement seam, over per-read selection from a
  candidate set the build emits, since the latter leaves typing in the run.
- *One budget over the tree — decided; its unit — open.* Cells, or nodes and
  bytes. Recommended: nodes and bytes, since a seam cell's size is its
  section's mints.
- *Who writes a table — decided.* A tenant step of the parent lays an entry
  and spawns the specialization as a `Fresh` child of that write home; a
  hit's stamp is a `Cell` written through the borrow, stated in the header's
  contract.
- *Pointers within, relay across — decided.* Pointers across owned, ordered
  tiers stay; a borrowed branch is a probe-only cache, and a node built over
  its content relays that content locally first, insert-if-absent, once per
  scratch lifetime.
- *Build cells on the ordinary step protocol — decided.* A binding reaches a
  specialization as its birth, adopted by the ancestry rule's forced copy;
  the frame's typing context is a read-only counted borrow of a header,
  carried apart from its placement.
- *The rest end and the read-only borrow as reshapes or new verbs — open.*
  `leave` extended to a cell that stays, or an end of its own; the `'lent`
  brand kept for reads alone, or dropped.
- *What the run still checks — open.* The irreducible residue once the build
  decides; a `MATCH` arm's `it` over an action's result in a hot loop is the
  case to construct.
- *The partition of `src/build` — open.* By what it produces: shapes, and
  facts by kind — typing, selection, class, key.
- *Shape cells for `EVAL` — open.* Segments numbered per shape cell rather
  than by one counter; a callable born in composed code keying under its
  shape cell.
- *What this absorbs — open.* [Activation typing](activation-typing.md),
  [views settled from content](content-settled-views.md),
  [every pinned path minted at the load](pins-minted-at-load.md) and
  cellgraph's [tenant's own scratch](../../cellgraph/roadmap/tenant-scratch.md)
  each describe a piece of this end state over today's mechanisms.

## Dependencies

[Type tiers](../../design/type-tiers.md) and
[the scheduler](../../src/scheduler/README.md#specializations) describe the
mechanisms this replaces.

**Requires:** none.

**Unblocks:**

- [Agents](../rewrite/agents.md) — the per-agent build arena.
